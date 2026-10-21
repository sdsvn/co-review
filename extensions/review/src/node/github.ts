import { spawn } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';
import { inject, injectable } from '@theia/core/shared/inversify';
import { parsePatchMeta } from '../common/patch';
import { Review, ReviewDecision, ReviewThread } from '../common/review-model';
import { GitHubEdits, GitHubPost, GitHubPreview, GitHubTarget } from '../common/review-protocol';
import { ReviewStore } from './review-store';

/** A review comment on a line of the pull request's diff (GitHub's review API shape, plus the thread it comes from). */
interface LineComment {
    threadId: string;
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
 * when it gave one (post_review_to_github's `comments`), else the reviewer's own messages; a finding they kept is the
 * finding's text followed by what they added to it. Other replies from agents (and who said what) stay in Co-Review.
 */
function threadBody(thread: ReviewThread, reworded?: string): string {
    const humans = thread.messages.filter(m => m.author.kind === 'human' && m.body.trim());
    const first = thread.messages.find(m => m.author.kind !== 'system' && m.body.trim());
    // An agent's finding the reviewer kept is the point itself; their replies add to it, they don't replace it.
    const finding = thread.origin === 'finding' && first && first.author.kind !== 'human' ? [first] : [];
    const kept = [...finding, ...humans];
    const text = reworded?.trim() || (kept.length ? kept.map(m => m.body.trim()).join('\n\n') : first?.body.trim() ?? '');
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
        // and the reviewer's questions to an agent ("Ask Agent": a conversation, not feedback for the author) stay in Co-Review.
        const reviewers = (t: ReviewThread) => t.intent !== 'question' && (t.origin === 'finding' || t.messages.some(m => m.author.kind === 'human'));
        const posted = (t: ReviewThread) => (t.status === 'open' && reviewers(t)) || t.proposal?.status === 'accepted';
        for (const thread of review.threads.filter(posted).sort((a, b) => a.number - b.number)) {
            const a = thread.location.patchAnchor;
            const side = a?.side === 'old' ? 'LEFT' : 'RIGHT';
            const threadId = thread.id;
            if (thread.location.kind === 'patch' && a?.path && a.type === 'code-line' && a.line) {
                comments.push({ threadId, path: a.path, line: a.line, side, body: threadBody(thread, reworded.get(thread.id)) });
            } else if (thread.location.kind === 'patch' && a?.path && a.type === 'code-range' && a.startLine && a.endLine) {
                comments.push(a.startLine === a.endLine
                    ? { threadId, path: a.path, line: a.endLine, side, body: threadBody(thread, reworded.get(thread.id)) }
                    : { threadId, path: a.path, line: a.endLine, side, start_line: a.startLine, start_side: side, body: threadBody(thread, reworded.get(thread.id)) });
            } else {
                const where = a?.path ?? (thread.location.docAnchor ? 'the description' : 'the pull request');
                elsewhere.push(`- **${where}:** ${threadBody(thread, reworded.get(thread.id)).replace(/\n/g, '\n  ')}`);
            }
        }
        const body = [summary.trim(), elsewhere.length ? `${summary.trim() ? '\n' : ''}${elsewhere.join('\n')}` : '']
            .filter(Boolean).join('\n') || (decision === 'approve' ? 'Looks good.' : '');
        return { comments, body, elsewhere: elsewhere.length };
    }

    /** Exactly what `publish` would send first (before any fallback GitHub needs), without posting it. */
    async preview(reviewId: string, decision?: ReviewDecision, summary?: string, reworded?: Map<string, string>): Promise<GitHubPreview> {
        const review = await this.store.get(reviewId);
        if (!review) {
            throw new Error('No review.');
        }
        const chosen = decision ?? review.verdict?.decision ?? 'comment';
        const { comments, body } = this.compose(review, chosen, summary ?? review.verdict?.summary ?? '', reworded);
        const own = chosen !== 'comment' && await this.ownPullRequest(await this.target(review));
        return {
            event: own ? 'COMMENT' : EVENTS[chosen], body: own ? asComment(chosen, body) : body, note: own ? ownNote(chosen) : undefined,
            comments: comments.map(c => ({ threadId: c.threadId, path: c.path, line: c.line, startLine: c.start_line, side: c.side, body: c.body }))
        };
    }

    /**
     * Whether the pull request is the signed-in GitHub user's own: GitHub refuses "Approve" and "Request changes" on it.
     * Undefined when `gh` can't tell (the post then finds out from GitHub's refusal).
     */
    protected async ownPullRequest(target: GitHubTarget | undefined): Promise<boolean | undefined> {
        if (!target) {
            return undefined;
        }
        const [author, me] = await Promise.all([
            this.gh(['api', `repos/${target.repo}/pulls/${target.number}`, '--jq', '.user.login'], ''),
            this.gh(['api', 'user', '--jq', '.login'], '')
        ]);
        return author.ok && me.ok && !!author.stdout.trim() ? author.stdout.trim() === me.stdout.trim() : undefined;
    }

    /**
     * Posts the review; GitHub's refusals that have a fallback (your own pull request, a line outside the diff) are retried.
     * `edits` are the reviewer's changes to its preview, posted as they wrote them.
     */
    async publish(reviewId: string, decision?: ReviewDecision, summary?: string, reworded?: Map<string, string>, edits?: GitHubEdits): Promise<GitHubPost> {
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
        // The reviewer's edits are of the preview, which already reads as a comment on their own pull request.
        let body = edits ? edits.body.trim() : composed.body;
        let comments = (edits ? composed.comments.map(c => ({ ...c, body: (edits.comments[c.threadId] ?? c.body).trim() })) : composed.comments)
            .filter(c => c.body);
        const posted = comments.length;
        const notes: string[] = [];
        const asOwn = () => {
            notes.push(ownNote(chosen));
            body = edits ? body : asComment(chosen, body);
            event = 'COMMENT';
        };
        if (event !== 'COMMENT' && await this.ownPullRequest(target)) {
            asOwn();
        }
        // Each fallback once: your own pull request, then lines no longer in the diff.
        let last = '';
        for (let attempt = 0; attempt < 3; attempt++) {
            const result = await this.gh(['api', '--method', 'POST', endpoint, '--input', '-'],
                JSON.stringify({ event, body, comments: comments.map(({ threadId, ...c }) => c) }));
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
                return { url, comments: comments.length, inBody: composed.elsewhere + (comments.length ? 0 : posted), event, notes };
            }
            // `gh api` puts GitHub's reason on stderr (`gh: <reason>`) and the response itself on stdout.
            const message = [result.stderr.trim(), result.stdout.trim()].filter(Boolean).join('\n');
            last = result.stderr.trim() || result.stdout.trim();
            console.error(`[co-review] GitHub refused the review of ${target.repo}#${target.number} (${event}, ${comments.length} line comments): ${message}`);
            if (/ENOENT|not found: gh|command not found/i.test(message)) {
                throw new Error('The GitHub CLI (`gh`) is not installed. Install it (https://cli.github.com) and run `gh auth login`.');
            }
            if (/gh auth login|authentication|HTTP 401/i.test(message)) {
                throw new Error('The GitHub CLI is not signed in: run `gh auth login`, then post again.');
            }
            if (event !== 'COMMENT' && /own pull request/i.test(message)) {
                // GitHub doesn't let you approve (or request changes on) your own pull request.
                asOwn();
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
            throw new Error(`GitHub refused the review: ${last.replace(/^gh: /, '').slice(0, 400)}`);
        }
        throw new Error(`GitHub refused the review: ${last.replace(/^gh: /, '').slice(0, 400)}`);
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

/** GitHub takes only a comment on your own pull request: the review's body says the decision instead. */
function asComment(decision: ReviewDecision, body: string): string {
    return `**${DECISION_LABEL[decision]}.** ${body}`.trim();
}

function ownNote(decision: ReviewDecision): string {
    return `GitHub doesn't allow "${DECISION_LABEL[decision]}" on your own pull request, so it is posted as a comment that says so.`;
}

/** A preview as Markdown, for a thread that asks the reviewer to confirm a post: each part quoted as it will appear. */
export function previewMarkdown(preview: GitHubPreview): string {
    const quote = (text: string) => text.split('\n').map(l => `> ${l}`).join('\n');
    const parts = [preview.body.trim() ? `**Review (${preview.event.toLowerCase().replace('_', ' ')}):**\n\n${quote(preview.body)}` : `**Review (${preview.event.toLowerCase().replace('_', ' ')}):** no message`];
    if (preview.note) {
        parts.push(`_${preview.note}_`);
    }
    for (const c of preview.comments) {
        parts.push(`**${c.path}:${c.startLine ? `${c.startLine}-` : ''}${c.line}${c.side === 'LEFT' ? ' (old side)' : ''}:**\n\n${quote(c.body)}`);
    }
    return parts.join('\n\n');
}
