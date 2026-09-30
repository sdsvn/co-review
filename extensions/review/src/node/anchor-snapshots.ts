import { FileUri } from '@theia/core/lib/common/file-uri';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import * as path from 'path';
import { rangeIn, offsetAt } from '../common/anchor-tracking';
import { CodeLocation, contentVersion, isOverviewPage } from '../common/review-model';

/** Where the snapshots of a workspace's commented files are: content-addressed by version, next to its reviews. */
export function snapshotsDir(workspaceDir: string): string {
    return path.join(workspaceDir, 'snapshots');
}

export function saveSnapshot(dir: string, version: string, text: string): void {
    const file = path.join(dir, version);
    if (!existsSync(file)) {
        mkdirSync(dir, { recursive: true });
        writeFileSync(file, text);
    }
}

/** Letters and digits only, for comparing rendered text (a document's) with its Markdown source. */
function plain(text: string): string {
    return text.replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
}

/**
 * The tracked target of a new comment (see CodeLocation.tracked), from the file as it is now, with a snapshot of that
 * version: a line or range comment's lines, or a document's quoted text found in its Markdown source (only when that
 * place is clear). Anything else, or a file that doesn't read as the comment says, is left as it is.
 */
export function trackNew(workspaceDir: string, location: CodeLocation): CodeLocation {
    if (location.tracked || !location.uri?.startsWith('file:') || isOverviewPage(location.uri)) {
        return location;
    }
    let text: string;
    try {
        text = readFileSync(FileUri.fsPath(location.uri), 'utf8');
    } catch {
        return location;
    }
    const version = contentVersion(text);
    let range: [number, number] | undefined;
    if ((location.kind === 'line' || location.kind === 'range' || location.kind === 'symbol') && location.range) {
        const from = offsetAt(text, location.range.start.line, location.range.start.character);
        const to = offsetAt(text, location.range.end.line, location.range.end.character);
        // The editor's text is the file's (an unsaved buffer isn't tracked from the disk).
        if (to > from && (!location.anchor?.text || text.slice(from, to) === location.anchor.text)) {
            range = [from, to];
        }
    } else if (location.kind === 'document' && location.docAnchor?.type === 'text' && location.docAnchor.exact) {
        range = sourceRange(text, location.docAnchor.exact, location.docAnchor.prefix ?? '', location.docAnchor.suffix ?? '');
    }
    if (!range) {
        return location;
    }
    saveSnapshot(snapshotsDir(workspaceDir), version, text);
    return { ...location, tracked: rangeIn(text, version, range[0], range[1]) };
}

/** Where rendered `exact` is in a Markdown `source`: its one occurrence, or the one whose context reads best. */
function sourceRange(source: string, exact: string, prefix: string, suffix: string): [number, number] | undefined {
    const places: { at: number; score: number }[] = [];
    for (let at = source.indexOf(exact); at >= 0; at = source.indexOf(exact, at + 1)) {
        const before = plain(source.slice(Math.max(0, at - 200), at));
        const after = plain(source.slice(at + exact.length, at + exact.length + 200));
        const p = plain(prefix);
        const s = plain(suffix);
        places.push({ at, score: (p && before.endsWith(p) ? 1 : 0) + (s && after.startsWith(s) ? 1 : 0) });
    }
    places.sort((a, b) => b.score - a.score);
    if (!places.length || (places[1] && places[1].score === places[0].score)) {
        return undefined;
    }
    return [places[0].at, places[0].at + exact.length];
}
