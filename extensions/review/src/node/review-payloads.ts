import { FileUri } from '@theia/core/lib/common/file-uri';
import * as path from 'path';
import { CodeLocation, DocAnchor, PatchAnchor, Review, ReviewThread } from '../common/review-model';
import { DOCUMENT_NAMES } from '../common/patch';

/**
 * The agent-facing shape of threads: short ids (`t<N>`), `target` (`doc` / `patch:<slug>`) and raw
 * anchors. Used by the MCP tools and the `review.json` state file.
 */

export type AnchorRecord = Record<string, any>;

export function threadRef(thread: ReviewThread): string {
    return `t${thread.number}`;
}

export function findThread(review: Review, id: string): ReviewThread | undefined {
    const m = id.match(/^t(\d+)$/);
    return review.threads.find(t => t.id === id || (m && t.number === Number(m[1])));
}

/**
 * Where a thread is: `doc` (the review directory's document), `doc:<path>` (another Markdown page of the
 * directory, e.g. a concept page of an OKF bundle), `patch:<slug>`, or `code`.
 */
export function targetOf(thread: ReviewThread, dir?: string): string {
    const l = thread.location;
    if (l.kind === 'document') {
        const rel = dir && l.uri ? path.relative(dir, FileUri.fsPath(l.uri)).split(path.sep).join('/') : undefined;
        return rel && !rel.startsWith('..') && !DOCUMENT_NAMES.includes(rel) ? `doc:${rel}` : 'doc';
    }
    if (l.kind === 'patch' && l.uri) {
        return `patch:${path.basename(FileUri.fsPath(l.uri)).replace(/\.(patch|diff)$/, '')}`;
    }
    return 'code';
}

/** The raw anchor of a thread (code threads map onto code-line / code-range / code-file). */
export function anchorOf(thread: ReviewThread, root?: string): AnchorRecord {
    const l = thread.location;
    if (l.docAnchor) {
        return l.docAnchor;
    }
    if (l.patchAnchor) {
        return l.patchAnchor;
    }
    const rel = l.uri && root ? path.relative(root, FileUri.fsPath(l.uri)) : undefined;
    if (l.range && rel) {
        const start = l.range.start.line + 1;
        const end = l.range.end.line + 1;
        return start === end
            ? { type: 'code-line', path: rel, line: start, side: 'new', source: l.anchor?.text }
            : { type: 'code-range', path: rel, startLine: start, endLine: end, side: 'new', source: l.anchor?.text };
    }
    return rel ? { type: 'code-file', path: rel } : { type: 'document' };
}

function where(a: AnchorRecord): string {
    return a.path ?? a.blockId ?? a.nodeId ?? 'document';
}

function line(a: AnchorRecord): number | undefined {
    return typeof a.line === 'number' ? a.line : typeof a.startLine === 'number' ? a.startLine : undefined;
}

/** One entry of `comments[]` (await_review / get_review / await_comment). */
export function commentOf(thread: ReviewThread, root: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
    const anchor = anchorOf(thread, root);
    const last = thread.messages[thread.messages.length - 1];
    return {
        id: threadRef(thread),
        threadId: thread.id,
        target: targetOf(thread, root),
        kind: anchor.type,
        where: where(anchor),
        line: line(anchor),
        side: anchor.side,
        severity: thread.severity,
        labels: thread.labels ?? [],
        status: thread.status,
        body: last?.body ?? '',
        source: anchor.source,
        origin: thread.origin,
        intent: thread.intent,
        ...(thread.proposal ? { suggestion: thread.proposal.after } : {}),
        ...extra
    };
}

/** Location of a `{target, anchor}` pair inside a review directory. */
export function locationFromAnchor(dir: string, docFile: string | undefined, target: string | undefined, anchor: AnchorRecord): CodeLocation {
    const t = target ?? (String(anchor.type).startsWith('code-') || anchor.type === 'patch' ? 'patch' : 'doc');
    if (t.startsWith('patch')) {
        const slug = t.split(':')[1];
        const file = slug ? patchFile(dir, slug) : undefined;
        return {
            kind: 'patch',
            uri: file ? FileUri.create(file).toString() : undefined,
            patchAnchor: anchor as PatchAnchor,
            anchor: { text: anchor.source ?? '' }
        };
    }
    // `doc:<path>`: another page of the review directory; it must stay inside it.
    const page = t.startsWith('doc:') ? path.resolve(dir, t.slice('doc:'.length)) : undefined;
    const file = page && page.startsWith(path.resolve(dir) + path.sep) ? page : docFile;
    return {
        kind: 'document',
        uri: file ? FileUri.create(file).toString() : undefined,
        docAnchor: anchor as DocAnchor,
        anchor: { text: anchor.exact ?? anchor.source ?? '' }
    };
}

export function patchFile(dir: string, slug: string): string {
    const fs = require('fs') as typeof import('fs');
    for (const ext of ['.patch', '.diff']) {
        const file = path.join(dir, slug + ext);
        if (fs.existsSync(file)) {
            return file;
        }
    }
    return path.join(dir, `${slug}.patch`);
}

function clock(ms: number): string {
    const d = new Date(ms);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** The review's state file (`review.json`), for tools that read the review from disk. */
export function toStateFile(review: Review, doc: { slug: string; title: string; markdown: string } | undefined): object {
    const root = review.bundle?.dir ?? FileUri.fsPath(review.workspaceRoot);
    return {
        doc: { slug: doc?.slug ?? '', title: doc?.title ?? review.title, version: review.bundle?.docVersion ?? 1, markdown: doc?.markdown ?? '' },
        threads: review.threads.map(t => ({
            id: threadRef(t),
            target: targetOf(t, root),
            anchor: anchorOf(t, root),
            status: t.status,
            severity: t.severity ?? 'medium',
            labels: t.labels ?? [],
            origin: t.origin,
            intent: t.intent,
            sourceId: t.sourceId,
            proposal: t.proposal,
            messages: t.messages.filter(m => m.status !== 'streaming').map(m => ({
                author: m.author.name, role: m.author.kind, time: clock(m.createdAt), body: m.body
            }))
        })),
        seq: review.nextThreadNumber - 1,
        review: review.verdict
            ? { decision: review.verdict.decision, summary: review.verdict.summary, submittedAt: clock(review.verdict.submittedAt), count: review.verdict.count }
            : { count: 0 }
    };
}

/** Accepted patch suggestions, for the agent to apply as commits. */
export function acceptedSuggestions(review: Review): object[] {
    return review.threads.filter(t => t.location.kind === 'patch' && t.proposal?.status === 'accepted').map(t => ({
        threadId: threadRef(t), path: t.proposal!.path ?? t.location.patchAnchor?.path, startLine: t.proposal!.startLine ?? t.location.patchAnchor?.line ?? t.location.patchAnchor?.startLine,
        before: t.proposal!.before, after: t.proposal!.after
    }));
}
