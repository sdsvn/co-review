import { spawn } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';
import { inject, injectable } from '@theia/core/shared/inversify';
import { parsePatchMeta } from '../common/patch';
import { Review, ReviewDecision, ReviewThread } from '../common/review-model';
import { GitHubPost, GitHubTarget } from '../common/review-protocol';
import { ReviewStore } from './review-store';

/** A review comment on a line of the pull request's diff (GitHub's review API shape). */
interface LineComment {
    path: string;
    line: number;
    side: 'RIGHT' | 'LEFT';
    start_line?: number;
    start_side?: 'RIGHT' | 'LEFT';
    body: string;
}

const EVENTS: Record<ReviewDecision, 'APPROVE' | 'REQUEST_CHANGES' | 'COMMENT'> = {
    approve: 'APPROVE', 'request-changes': 'REQUEST_CHANGES', comment: 'COMMENT'
};
const DECISION_LABEL: Record<ReviewDecision, string> = { approve: 'Approve', 'request-changes': 'Request changes', comment: 'Comment' };

/**
 * A thread as the reviewer's comment on GitHub: their point, not the conversation. The agent's rewording in their voice
 * when it gave one (post_review_to_github's `comments`), else the reviewer's own messages; a finding they accepted
 * without writing anything is the finding's text. Replies from agents (and who said what) stay in Co-Review.
 */
function threadBody(thread: ReviewThread, reworded?: string): string {
    const humans = thread.messages.filter(m => m.author.kind === 'human' && m.body.trim());
    const first = thread.messages.find(m => m.author.kind !== 'system' && m.body.trim());
    const text = reworded?.trim() || (humans.length ? humans.map(m => m.body.trim()).join('\n\n') : first?.body.trim() ?? '');
    const suggestion = thread.proposal && thread.proposal.status !== 'rejected' && thread.location.patchAnchor?.side !== 'old'
        ? `\n\n\`\`\`suggestion\n${thread.proposal.after.replace(/\n$/, '')}\n\`\`\`` : '';
    return (text || '(no message)') + suggestion;
}

/**
 * Posts a review of a pull request (a review directory whose `PR.md` names the GitHub repository and number) to
 * GitHub as one pull-request review, through the GitHub CLI (`gh api`, so `gh auth login` is the only setup):
 * open threads on diff lines become line comments (a suggested edit becomes a GitHub suggestion), the rest goes into
 * the review's body, and the verdict becomes the review's event. Proposed findings nobody accepted and resolved
 * threads are left out, except accepted suggestions, which go out as GitHub suggestions.
 */
@injectable()
export class GitHubReviews {

    @inject(ReviewStore) protected readonly store: ReviewStore;

    /** The pull request a review belongs to, from its review directory's `PR.md` (or `<patch>.md`). */
    async target(review: Review | undefined): Promise<GitHubTarget | undefined> {
        const dir = review?.bundle?.dir;
        if (!dir) {
            return undefined;
        }
        const names = await fs.readdir(dir).catch(() => [] as string[]);
        const candidates = ['PR.md', ...names.filter(n => /\.(patch|diff)$/.test(n)).map(n => n.replace(/\.(patch|diff)$/, '.md'))];
        for (const name of candidates) {
            const text = await fs.readFile(path.join(dir, name), 'utf8').catch(() => undefined);
            const meta = text !== undefined ? parsePatchMeta(text) : undefined;
            if (meta?.number && meta.repo && /^[\w.-]+\/[\w.-]+$/.test(meta.repo)) {
                return {
                    repo: meta.repo, number: meta.number, url: meta.url ?? `https://github.com/${meta.repo}/pull/${meta.number}`,
                    postedRound: review!.bundle?.github?.round, postedUrl: review!.bundle?.github?.url
                };
            }
        }
        return undefined;
    }

    /** What would be posted: line comments and the body (with the comments that aren't on a diff line). */
    compose(review: Review, decision: ReviewDecision, summary: string, reworded: Map<string, string> = new Map()): { comments: LineComment[]; body: string; elsewhere: number } {
        const comments: LineComment[] = [];
        const elsewhere: string[] = [];
        // What the reviewer stands behind: open threads they wrote in or findings they accepted, and suggested edits they
        // accepted (accepting one resolves its thread). An agent's own threads (its questions, this post's confirmation)
        // stay in Co-Review.
        const reviewers = (t: ReviewThread) => t.origin === 'finding' || t.messages.some(m => m.author.kind === 'human');
        const posted = (t: ReviewThread) => (t.status === 'open' && reviewers(t)) || t.proposal?.status === 'accepted';
        for (const thread of review.threads.filter(posted).sort((a, b) => a.number - b.number)) {
            const a = thread.location.patchAnchor;
            const side = a?.side === 'old' ? 'LEFT' : 'RIGHT';
            if (thread.location.kind === 'patch' && a?.path && a.type === 'code-line' && a.line) {
                comments.push({ path: a.path, line: a.line, side, body: threadBody(thread, reworded.get(thread.id)) });
            } else if (thread.location.kind === 'patch' && a?.path && a.type === 'code-range' && a.startLine && a.endLine) {
                comments.push(a.startLine === a.endLine
                    ? { path: a.path, line: a.endLine, side, body: threadBody(thread, reworded.get(thread.id)) }
                    : { path: a.path, line: a.endLine, side, start_line: a.startLine, start_side: side, body: threadBody(thread, reworded.get(thread.id)) });
            } else {
                const where = a?.path ?? (thread.location.docAnchor ? 'the description' : 'the pull request');
                elsewhere.push(`- **${where}:** ${threadBody(thread, reworded.get(thread.id)).replace(/\n/g, '\n  ')}`);
            }
        }
        const body = [summary.trim(), elsewhere.length ? `${summary.trim() ? '\n' : ''}${elsewhere.join('\n')}` : '']
            .filter(Boolean).join('\n') || (decision === 'approve' ? 'Looks good.' : '');
        return { comments, body, elsewhere: elsewhere.length };
    }

    /** Posts the review; GitHub's refusals that have a fallback (your own pull request, a line outside the diff) are retried. */
    async publish(reviewId: string, decision?: ReviewDecision, summary?: string, reworded?: Map<string, string>): Promise<GitHubPost> {
        const review = await this.store.get(reviewId);
        const target = await this.target(review);
        if (!review || !target) {
            throw new Error('This review is not a GitHub pull request: its review directory needs a PR.md with `repo: <owner>/<name>` '
                + 'and `pr: <number>` (or `url: <pull request URL>`).');
        }
        const chosen = decision ?? review.verdict?.decision ?? 'comment';
        const composed = this.compose(review, chosen, summary ?? review.verdict?.summary ?? '', reworded);
        const endpoint = `repos/${target.repo}/pulls/${target.number}/reviews`;
        let event = EVENTS[chosen];
        let body = composed.body;
        let comments = composed.comments;
        const notes: string[] = [];
        for (let attempt = 0; attempt < 3; attempt++) {
            const result = await this.gh(['api', '--method', 'POST', endpoint, '--input', '-'], JSON.stringify({ event, body, comments }));
            if (result.ok) {
                const url = (() => {
                    try {
                        return (JSON.parse(result.stdout) as { html_url?: string }).html_url;
                    } catch {
                        return undefined;
                    }
                })() ?? target.url;
                const bundle = review.bundle!;
                await this.store.setBundle(review.id, { ...bundle, github: { url, round: review.verdict?.count ?? 0, postedAt: Date.now() } });
                return { url, comments: comments.length, inBody: composed.elsewhere + (comments.length ? 0 : composed.comments.length), event, notes };
            }
            const message = result.stderr || result.stdout;
            if (/ENOENT|not found: gh|command not found/i.test(message)) {
                throw new Error('The GitHub CLI (`gh`) is not installed. Install it (https://cli.github.com) and run `gh auth login`.');
            }
            if (/gh auth login|authentication|HTTP 401/i.test(message)) {
                throw new Error('The GitHub CLI is not signed in: run `gh auth login`, then post again.');
            }
            if (event !== 'COMMENT' && /own pull request/i.test(message)) {
                // GitHub doesn't let you approve (or request changes on) your own pull request.
                notes.push(`GitHub doesn't allow "${DECISION_LABEL[chosen]}" on your own pull request, so it was posted as a comment.`);
                body = `**${DECISION_LABEL[chosen]}.** ${body}`.trim();
                event = 'COMMENT';
                continue;
            }
            if (comments.length && /HTTP 422|Unprocessable|line|position|diff/i.test(message)) {
                // A line GitHub can't place in the diff (it changed since the review started): put every comment in the body.
                notes.push('Some lines are no longer in the pull request\'s diff, so the line comments were added to the review\'s body.');
                body = [body, ...comments.map(c => `- **${c.path}:${c.start_line ? `${c.start_line}-` : ''}${c.line}:** ${c.body.replace(/\n/g, '\n  ')}`)]
                    .filter(Boolean).join('\n');
                comments = [];
                continue;
            }
            throw new Error(`GitHub refused the review: ${message.trim().slice(0, 400)}`);
        }
        throw new Error('GitHub refused the review.');
    }

    protected gh(args: string[], input: string): Promise<{ ok: boolean; stdout: string; stderr: string }> {
        return new Promise(resolve => {
            const child = spawn('gh', args, { stdio: ['pipe', 'pipe', 'pipe'], env: process.env });
            let stdout = '';
            let stderr = '';
            child.stdout.on('data', d => (stdout += d));
            child.stderr.on('data', d => (stderr += d));
            child.on('error', error => resolve({ ok: false, stdout, stderr: `${stderr}${error.message}` }));
            child.on('close', code => resolve({ ok: code === 0, stdout, stderr }));
            child.stdin.on('error', () => undefined);
            child.stdin.end(input);
        });
    }
}
