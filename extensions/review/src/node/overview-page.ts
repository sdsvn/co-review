import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { promises as fs } from 'fs';
import * as path from 'path';
import { DocAnchor, isOverviewPage, Review, ReviewThread } from '../common/review-model';
import { parsePatch, parsePatchMeta, PatchMeta } from '../common/patch';
import { BundleService } from './bundle-service';
import { RepoIndex } from './repo-index';
import { ReviewStore } from './review-store';

const STATUS: Record<string, string> = { proposed: 'proposed — accept or dismiss it', open: 'open', resolved: 'resolved' };

/** Where a thread is, as a link the rendered page opens (code at a line, a file, a folder or the repository). */
function where(thread: ReviewThread, root: string): { label: string; href?: string } {
    const l = thread.location;
    if (l.patchAnchor) {
        const a = l.patchAnchor;
        const line = a.line ?? a.startLine;
        return { label: `${a.path ?? path.basename(l.uri ? FileUri.fsPath(l.uri) : 'patch')}${line ? `:${line}` : ''}` };
    }
    if (l.docAnchor) {
        return { label: `the document, ${DocAnchor.describe(l.docAnchor)}` };
    }
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

/** A section written elsewhere, under this page's `##`: its title dropped, its headings one level down. */
function nest(markdown: string): string[] {
    let inFence = false;
    return markdown.replace(/^#\s+.*\n+/, '').split('\n').map(line => {
        if (/^(```|~~~)/.test(line)) {
            inFence = !inFence;
        }
        return !inFence && /^#{1,5}\s/.test(line) ? `#${line}` : line;
    });
}

/** What goes on a review's overview page besides the review itself. */
export interface OverviewParts {
    /** The pull request, from the review directory's `PR.md`. */
    pr?: PatchMeta;
    /** The reviewed patches: name, and per file the lines added and removed. */
    patches: { name: string; files: { path: string; added: number; removed: number }[] }[];
    /** A repository review: the repository overview (where to start, the areas and how they connect). */
    repository?: string;
}

/**
 * A review's front page, the first thing the reviewer reads, for every kind of review: the pull request, the agent's
 * overview of the change or repository (what it does, how it works as a Mermaid chart, what else it affects), the
 * change itself (files, lines), then every finding and comment grouped by area, each with its severity, status, a
 * link to the code, the code and the conversation so far, and for a repository review the repository's areas.
 */
export function overviewPage(review: Review, root: string, parts: OverviewParts): string {
    const threads = review.threads.filter(t => !(t.location.kind === 'document' && isOverviewPage(t.location.uri)));
    // A thread's area: its first label, else the file it is on in a patch, else its top-level folder.
    const areaOf = (t: ReviewThread) => t.labels?.[0] ?? t.location.patchAnchor?.path
        ?? (t.location.docAnchor ? 'The document' : where(t, root).label.includes('/') ? where(t, root).label.split('/')[0] : 'Repository');
    const areas = new Map<string, ReviewThread[]>();
    for (const thread of [...threads].sort((a, b) => a.number - b.number)) {
        areas.set(areaOf(thread), [...areas.get(areaOf(thread)) ?? [], thread]);
    }
    const count = (status: string) => threads.filter(t => t.status === status).length;
    const agent = review.agent?.name;
    const lines = [`# ${review.title}`, ''];
    const verdict = review.verdict ? ` Round ${review.verdict.count} submitted: ${review.verdict.decision}.` : '';
    lines.push(threads.length
        ? `${threads.length} thread${threads.length === 1 ? '' : 's'}${agent ? `, first pass by ${agent}` : ''}: `
        + `${count('proposed')} proposed, ${count('open')} open, ${count('resolved')} resolved.${verdict} `
        + 'Accept or dismiss findings in the Review panel, open one with its link, or comment on this page.'
        : `No findings yet.${verdict}`);

    if (parts.pr) {
        const pr = parts.pr;
        lines.push('', '## The pull request', '', [
            pr.url ? `[${pr.title ?? `#${pr.number ?? ''}`}](${pr.url})` : pr.title,
            pr.branch && pr.base ? `\`${pr.branch}\` into \`${pr.base}\`` : undefined,
            pr.commits ? `${pr.commits} commit${pr.commits === 1 ? '' : 's'}` : undefined
        ].filter(Boolean).join(' · '));
        if (pr.body.trim()) {
            lines.push('', ...nest(pr.body.trim()));
        }
    }

    lines.push('', review.bundle && parts.patches.length ? '## What the change does' : '## Overview', '');
    lines.push(...review.overview?.trim()
        ? nest(review.overview.trim())
        : [`_${agent ?? 'The agent'} has not written an overview${agent ? '' : ' yet'}: `
            + 'what this does, how it works (a flow chart) and what else it affects._']);

    for (const patch of parts.patches) {
        const added = patch.files.reduce((n, f) => n + f.added, 0);
        const removed = patch.files.reduce((n, f) => n + f.removed, 0);
        lines.push('', parts.patches.length > 1 ? `## The change: ${patch.name}` : '## The change', '',
            `${patch.files.length} file${patch.files.length === 1 ? '' : 's'}, +${added} −${removed}.`, '');
        for (const file of patch.files) {
            lines.push(`- \`${file.path}\` +${file.added} −${file.removed}`);
        }
    }

    lines.push('', `## Findings and comments (${threads.length})`);
    if (!threads.length) {
        lines.push('', 'None yet.');
    }
    for (const [area, list] of areas) {
        lines.push('', `### ${area} (${list.length})`);
        for (const thread of list) {
            const at = where(thread, root);
            const first = thread.messages[0];
            lines.push('', `#### #${thread.number} ${first?.body.split('\n')[0].replace(/^#+\s*/, '').slice(0, 90) ?? ''}`, '');
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
            for (const message of thread.messages.filter(m => m.body.trim())) {
                lines.push('', ...`**${message.author.name}:** ${message.body.trim()}`.split('\n').map(line => `> ${line}`));
            }
        }
    }

    if (parts.repository) {
        lines.push('', '## The repository', '', ...nest(parts.repository));
    }
    return lines.join('\n') + '\n';
}

/** Writes review overview pages and keeps every written one up to date as its review changes. */
@injectable()
export class OverviewPages {

    @inject(ReviewStore) protected readonly store: ReviewStore;
    @inject(BundleService) protected readonly bundles: BundleService;
    @inject(RepoIndex) protected readonly index: RepoIndex;

    protected readonly timers = new Map<string, NodeJS.Timeout>();
    /** Repository sections by review: built when the page is opened, reused when the review changes (it is slow). */
    protected readonly repositories = new Map<string, string>();

    @postConstruct()
    protected init(): void {
        this.store.onDidChange(change => {
            if (change.kind === 'deleted') {
                clearTimeout(this.timers.get(change.reviewId));
                this.timers.delete(change.reviewId);
                this.repositories.delete(change.reviewId);
                fs.rm(path.dirname(this.fileOf(change.workspaceRoot, change.reviewId)), { recursive: true, force: true }).catch(() => undefined);
            } else {
                // A page that was written (in this run or an earlier one) follows the review. Changes come in bursts
                // (an agent streaming a reply): write once per burst.
                const id = change.review.id;
                clearTimeout(this.timers.get(id));
                this.timers.set(id, setTimeout(() => {
                    this.timers.delete(id);
                    fs.access(this.fileOf(change.review.workspaceRoot, id))
                        .then(() => this.write(id, false), () => undefined)
                        .catch(e => console.error('[co-review] overview page failed', e));
                }, 500));
            }
        });
    }

    protected fileOf(workspaceRoot: string, reviewId: string): string {
        return path.join(this.store.workspaceDir(workspaceRoot), 'pages', reviewId, 'overview.md');
    }

    /**
     * Writes the review's overview page; returns its file URI. Undefined for a review directory without patches (a
     * design or a knowledge bundle): its document is its front page. `fresh`: rebuild the repository section.
     */
    async write(reviewId: string, fresh = true): Promise<string | undefined> {
        const review = await this.store.get(reviewId);
        const dir = review?.bundle?.dir;
        const patches = dir ? this.bundles.patchesOf(dir) : [];
        if (!review || (dir && !patches.length)) {
            return undefined;
        }
        const parts: OverviewParts = { patches: [] };
        if (dir) {
            const meta = await fs.readFile(path.join(dir, 'PR.md'), 'utf8').then(parsePatchMeta, () => undefined);
            parts.pr = meta && (meta.title || meta.url || meta.number) ? meta : undefined;
            for (const file of patches) {
                const text = await fs.readFile(file, 'utf8').catch(() => '');
                parts.patches.push({ name: path.basename(file).replace(/\.(patch|diff)$/, ''), files: parsePatch(text) });
            }
        } else {
            const root = FileUri.fsPath(review.workspaceRoot);
            if (fresh || !this.repositories.has(review.id)) {
                this.repositories.set(review.id, await this.index.overview(root, review.viewed).catch(() => ''));
            }
            parts.repository = this.repositories.get(review.id) || undefined;
        }
        const file = this.fileOf(review.workspaceRoot, review.id);
        await fs.mkdir(path.dirname(file), { recursive: true });
        await fs.writeFile(file, overviewPage(review, dir ?? FileUri.fsPath(review.workspaceRoot), parts));
        return FileUri.create(file).toString();
    }
}
