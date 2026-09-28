import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { promises as fs } from 'fs';
import * as path from 'path';
import { Review, ReviewThread } from '../common/review-model';
import { ReviewStore } from './review-store';

const STATUS: Record<string, string> = { proposed: 'proposed — accept or dismiss it', open: 'open', resolved: 'resolved' };

/** Where a thread is, as a link the rendered page opens (code at a line, a file, a folder or the repository). */
function where(thread: ReviewThread, root: string): { label: string; href?: string } {
    const l = thread.location;
    const file = l.uri?.startsWith('file:') ? path.relative(root, FileUri.fsPath(l.uri)).split(path.sep).join('/') : undefined;
    if (!file || file.startsWith('..')) {
        return { label: 'the repository' };
    }
    const line = l.range ? l.range.start.line + 1 : undefined;
    const end = l.range && l.range.end.line + 1 !== line ? l.range.end.line + 1 : undefined;
    const label = `${file}${line ? `:${line}${end ? `-${end}` : ''}` : ''}`;
    return { label, href: label };
}

function fence(code: string): string {
    const lines = code.replace(/\s+$/, '').split('\n');
    const shown = lines.length > 12 ? [...lines.slice(0, 12), `… ${lines.length - 12} more lines`] : lines;
    const ticks = shown.some(s => s.includes('```')) ? '````' : '```';
    return `${ticks}\n${shown.join('\n')}\n${ticks}`;
}

/**
 * A repository review's findings and comments as one Markdown page: grouped by area (a thread's first label, else its
 * top-level folder), each with its severity, status, a link to the code, the code and the conversation so far. It is
 * the review's front page: the reviewer reads it top to bottom and comments on it like any document.
 */
export function findingsPage(review: Review, root: string): string {
    const threads = review.threads.filter(t => t.location.kind !== 'document');
    const areaOf = (t: ReviewThread) => t.labels?.[0] ?? (where(t, root).label.includes('/') ? where(t, root).label.split('/')[0] : 'Repository');
    const areas = new Map<string, ReviewThread[]>();
    for (const thread of [...threads].sort((a, b) => a.number - b.number)) {
        areas.set(areaOf(thread), [...areas.get(areaOf(thread)) ?? [], thread]);
    }
    const count = (status: string) => threads.filter(t => t.status === status).length;
    const agent = review.agent?.name;
    const lines = [
        `# ${review.title}`,
        '',
        threads.length
            ? `${threads.length} thread${threads.length === 1 ? '' : 's'}${agent ? `, first pass by ${agent}` : ''}: `
            + `${count('proposed')} proposed, ${count('open')} open, ${count('resolved')} resolved. `
            + 'Accept or dismiss findings in the Review panel, open one with its link, or comment on this page.'
            : 'No findings yet.'
    ];
    for (const [area, list] of areas) {
        lines.push('', `## ${area} (${list.length})`);
        for (const thread of list) {
            const at = where(thread, root);
            const first = thread.messages[0];
            lines.push('', `### #${thread.number} ${first?.body.split('\n')[0].replace(/^#+\s*/, '').slice(0, 90) ?? ''}`, '');
            lines.push([
                thread.severity ? `**${thread.severity}**` : undefined,
                STATUS[thread.status] ?? thread.status,
                at.href ? `[${at.label}](${at.href})` : at.label,
                thread.location.symbol ? `\`${thread.location.symbol}\`` : undefined
            ].filter(Boolean).join(' · '));
            if (thread.location.anchor?.text?.trim()) {
                lines.push('', fence(thread.location.anchor.text));
            }
            // Quoted, so a message's own headings don't break the page's sections.
            for (const message of thread.messages) {
                lines.push('', ...`**${message.author.name}:** ${message.body.trim()}`.split('\n').map(line => `> ${line}`));
            }
        }
    }
    return lines.join('\n') + '\n';
}

/** Writes review findings pages and keeps every written one up to date as its review changes. */
@injectable()
export class FindingsPages {

    @inject(ReviewStore) protected readonly store: ReviewStore;

    protected readonly timers = new Map<string, NodeJS.Timeout>();

    @postConstruct()
    protected init(): void {
        this.store.onDidChange(change => {
            if (change.kind === 'deleted') {
                clearTimeout(this.timers.get(change.reviewId));
                this.timers.delete(change.reviewId);
                fs.rm(path.dirname(this.fileOf(change.workspaceRoot, change.reviewId)), { recursive: true, force: true }).catch(() => undefined);
            } else if (!change.review.bundle) {
                // A page that was written (in this run or an earlier one) follows the review. Changes come in bursts
                // (an agent streaming a reply): write once per burst.
                const id = change.review.id;
                clearTimeout(this.timers.get(id));
                this.timers.set(id, setTimeout(() => {
                    this.timers.delete(id);
                    fs.access(this.fileOf(change.review.workspaceRoot, id))
                        .then(() => this.write(id), () => undefined)
                        .catch(e => console.error('[co-review] findings page failed', e));
                }, 500));
            }
        });
    }

    protected fileOf(workspaceRoot: string, reviewId: string): string {
        return path.join(this.store.workspaceDir(workspaceRoot), 'findings', reviewId, 'findings.md');
    }

    /** Writes the review's findings page; returns its file URI (undefined for a review directory, which has its own pages). */
    async write(reviewId: string): Promise<string | undefined> {
        const review = await this.store.get(reviewId);
        if (!review || review.bundle) {
            return undefined;
        }
        const file = this.fileOf(review.workspaceRoot, review.id);
        await fs.mkdir(path.dirname(file), { recursive: true });
        await fs.writeFile(file, findingsPage(review, FileUri.fsPath(review.workspaceRoot)));
        return FileUri.create(file).toString();
    }
}
