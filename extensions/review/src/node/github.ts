import { spawn } from 'child_process';
import { promises as fs } from 'fs';
import * as path from 'path';
import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { parsePatch, parsePatchMeta } from '../common/patch';
import { Review, ReviewDecision, ReviewMessage, ReviewThread } from '../common/review-model';
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
 * A thread as the reviewer's comment on GitHub: their point, not a transcript. The agent's rewording in their voice
 * when it gave one (post_review_to_github's `comments`), else the thread from the reviewer's side: a finding they kept,
 * their messages and the agent's answers to them in order, so a question goes out with its answer and a change the
 * agent spelled out goes out with the reviewer's "yes" (who said what stays in Co-Review). The reviewer edits it in
 * the preview before it is posted.
 */
function threadBody(thread: ReviewThread, reworded?: string): string {
    const said = (m: ReviewMessage) => m.author.kind !== 'system' && !!m.body.trim() && !m.permission && (!m.status || m.status === 'done');
    const first = thread.messages.find(said);
    const joined = thread.messages.findIndex(m => m.author.kind === 'human' && said(m));
    // An agent's finding the reviewer kept is the point itself; their replies (and the answers they got) add to it.
    const kept = thread.messages.filter((m, i) => said(m)
        && (m.author.kind === 'human' || (joined >= 0 && i > joined) || (m === first && thread.origin === 'finding')));
    const text = reworded?.trim() || (kept.length ? kept.map(m => m.body.trim()).join('\n\n') : first?.body.trim() ?? '');
    return (text || '(no message)') + edit(thread);
}

/**
 * A suggested edit as GitHub shows it. A GitHub suggestion replaces the comment's whole lines, so it is one only when the
 * edit's `before` is exactly those lines; any other edit (part of a line, other lines) goes out as a diff to read.
 */
function edit(thread: ReviewThread): string {
    const proposal = thread.proposal;
    if (!proposal || proposal.status === 'rejected') {
        return '';
    }
    const lines = (text: string | undefined) => text?.replace(/\r\n/g, '\n').replace(/\n$/, '');
    const anchored = lines(thread.location.patchAnchor?.source ?? thread.location.anchor?.text);
    if (thread.location.patchAnchor?.side !== 'old' && anchored !== undefined && lines(proposal.before) === anchored) {
        return `\n\n\`\`\`suggestion\n${lines(proposal.after)}\n\`\`\``;
    }
    const diff = [...lines(proposal.before)!.split('\n').map(l => `-${l}`), ...lines(proposal.after)!.split('\n').map(l => `+${l}`)];
    return `\n\n\`\`\`diff\n${diff.join('\n')}\n\`\`\``;
}

/** A comment as a line of the review's body: where it is, then what it says. */
function inBody(c: { path: string; line: number; start_line?: number; body: string }): string {
    return `- **${c.path}:${c.start_line ? `${c.start_line}-` : ''}${c.line}:** ${c.body.replace(/\n/g, '\n  ')}`;
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
            } else if (!a && this.onCode(review, thread)) {
                comments.push({ threadId, ...this.onCode(review, thread)!, body: threadBody(thread, reworded.get(thread.id)) });
            } else {
                const where = a?.path ?? this.codePath(review, thread.location.uri) ?? (thread.location.docAnchor ? 'the description' : 'the pull request');
                elsewhere.push(`- **${where}:** ${threadBody(thread, reworded.get(thread.id)).replace(/\n/g, '\n  ')}`);
            }
        }
        const body = [summary.trim(), elsewhere.length ? `${summary.trim() ? '\n' : ''}${elsewhere.join('\n')}` : '']
            .filter(Boolean).join('\n') || (decision === 'approve' ? 'Looks good.' : '');
        return { comments, body, elsewhere: elsewhere.length };
    }

    /** A file of the change's code (its head checkout) as a path in the pull request, if `uri` is one. */
    protected codePath(review: Review, uri: string | undefined): string | undefined {
        const code = review.bundle?.code;
        if (!code || !uri?.startsWith('file:')) {
            return undefined;
        }
        const relative = path.relative(code, FileUri.fsPath(uri));
        return relative && !relative.startsWith('..') && !path.isAbsolute(relative) ? relative.split(path.sep).join('/') : undefined;
    }

    /**
     * Where a comment on the change's code (a line or lines of a file in its head checkout, not of the patch page) is in
     * the pull request: the head's lines, GitHub's right side. Only for a checkout of a known head commit.
     */
    protected onCode(review: Review, thread: ReviewThread): Pick<LineComment, 'path' | 'line' | 'side' | 'start_line' | 'start_side'> | undefined {
        const { kind, range } = thread.location;
        const file = review.bundle?.head ? this.codePath(review, thread.location.uri) : undefined;
        if (!file || !range || (kind !== 'line' && kind !== 'range' && kind !== 'symbol')) {
            return undefined;
        }
        const start = range.start.line + 1;
        // A selection to the start of a line ends on the line before it.
        const end = range.end.character === 0 && range.end.line > range.start.line ? range.end.line : range.end.line + 1;
        return end > start ? { path: file, line: end, side: 'RIGHT', start_line: start, start_side: 'RIGHT' } : { path: file, line: end, side: 'RIGHT' };
    }

    /**
     * Line comments GitHub can place: both ends in the same hunk of the pull request's diff as it is now, on their side.
     * The others go into the body, where they are kept with their place. Unchecked (all kept) if the diff can't be read.
     */
    protected async fit(target: GitHubTarget | undefined, comments: LineComment[]): Promise<{ comments: LineComment[]; off: LineComment[] }> {
        const files = target && comments.length ? await this.gh(['api', '--paginate', `repos/${target.repo}/pulls/${target.number}/files`,
            '--jq', '.[] | {filename, patch} | tojson'], '') : undefined;
        if (!files?.ok) {
            return { comments, off: [] };
        }
        // Per file and side: line → the hunk it is in.
        const hunks = new Map<string, Map<number, number>>();
        for (const json of files.stdout.split('\n').filter(Boolean)) {
            const { filename, patch } = JSON.parse(json) as { filename: string; patch?: string };
            const [file] = parsePatch(`diff --git a/${filename} b/${filename}\n${patch ?? ''}`);
            const left = new Map<number, number>();
            const right = new Map<number, number>();
            let hunk = 0;
            for (const row of file?.rows ?? []) {
                if (row.t === 'hunk') {
                    hunk++;
                }
                row.o !== undefined && left.set(row.o, hunk);
                row.n !== undefined && right.set(row.n, hunk);
            }
            hunks.set(`LEFT ${filename}`, left);
            hunks.set(`RIGHT ${filename}`, right);
        }
        const placeable = (c: LineComment) => {
            const lines = hunks.get(`${c.side} ${c.path}`);
            const hunk = lines?.get(c.line);
            return hunk !== undefined && (c.start_line === undefined || lines!.get(c.start_line) === hunk);
        };
        return { comments: comments.filter(placeable), off: comments.filter(c => !placeable(c)) };
    }

    /** Exactly what `publish` would send first (before any fallback GitHub needs), without posting it. */
    async preview(reviewId: string, decision?: ReviewDecision, summary?: string, reworded?: Map<string, string>): Promise<GitHubPreview> {
        const review = await this.store.get(reviewId);
        if (!review) {
            throw new Error('No review.');
        }
        const chosen = decision ?? review.verdict?.decision ?? 'comment';
        const composed = this.compose(review, chosen, summary ?? review.verdict?.summary ?? '', reworded);
        const target = await this.target(review);
        const { comments, off } = await this.fit(target, composed.comments);
        const body = [composed.body, ...off.map(inBody)].filter(Boolean).join('\n');
        const own = chosen !== 'comment' && await this.ownPullRequest(target);
        const note = [own ? ownNote(chosen) : '', off.length ? offNote(off.length) : ''].filter(Boolean).join(' ') || undefined;
        return {
            event: own ? 'COMMENT' : EVENTS[chosen], body: own ? asComment(chosen, body) : body, note,
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
        const notes: string[] = [];
        // Comments GitHub can't place on its diff go into the body (the preview, which the edits are of, has them there).
        const fitted = await this.fit(target, composed.comments);
        if (fitted.off.length) {
            notes.push(offNote(fitted.off.length));
        }
        // The reviewer's edits are of the preview, which already reads as a comment on their own pull request.
        let body = edits ? edits.body.trim() : [composed.body, ...fitted.off.map(inBody)].filter(Boolean).join('\n');
        let comments = (edits ? fitted.comments.map(c => ({ ...c, body: (edits.comments[c.threadId] ?? c.body).trim() })) : fitted.comments)
            .filter(c => c.body);
        const posted = comments.length;
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
                return { url, comments: comments.length, inBody: composed.elsewhere + fitted.off.length + (comments.length ? 0 : posted), event, notes };
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
            // Lines GitHub still couldn't place (the diff couldn't be read, or changed just now): every comment into the body.
            if (comments.length && /HTTP 422|Unprocessable/i.test(message) && /pull_request_review_thread|line|position|diff|path/i.test(message)) {
                notes.push('GitHub couldn\'t place the line comments on the pull request\'s diff, so they were added to the review\'s body.');
                body = [body, ...comments.map(inBody)].filter(Boolean).join('\n');
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

function offNote(count: number): string {
    return count === 1 ? 'One comment is on lines that are not in the pull request\'s diff, so it is in the review\'s body.'
        : `${count} comments are on lines that are not in the pull request's diff, so they are in the review's body.`;
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
