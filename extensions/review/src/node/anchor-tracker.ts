import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { promises as fs } from 'fs';
import * as path from 'path';
import { CodeLocation, contentVersion, Review } from '../common/review-model';
import { Edit, editsBetween, positionAt, track, TrackedRange } from '../common/anchor-tracking';
import { saveSnapshot, snapshotsDir } from './anchor-snapshots';
import { ReviewStore } from './review-store';

/**
 * Keeps every tracked comment of a review on what happened to its code: when a commented file changed since the
 * version its anchor is in, the anchor is carried through the edits (anchor-tracking.ts) from the snapshot of that
 * version, re-based on the current one, and the thread's range, quote and state follow. Resolved here, once, for the
 * panel, the editor, the document view and agents alike. Snapshots no comment points to any more are deleted.
 */
@injectable()
export class AnchorTracker {

    @inject(ReviewStore) protected readonly store: ReviewStore;

    /** Resolutions of one review at a time, so two readers don't both re-base it. */
    protected readonly running = new Map<string, Promise<void>>();

    /** Brings a review's anchors up to date with its files; cheap when nothing changed (one hash per commented file). */
    resolve(reviewId: string): Promise<void> {
        const pending = this.running.get(reviewId);
        if (pending) {
            return pending;
        }
        const run = this.doResolve(reviewId).catch(e => console.error('[co-review] tracking comments failed', e)).finally(() => this.running.delete(reviewId));
        this.running.set(reviewId, run);
        return run;
    }

    protected async doResolve(reviewId: string, explicit?: { uri: string; edits: Edit[]; before: string }): Promise<void> {
        const review = await this.store.get(reviewId);
        if (!review) {
            return;
        }
        const dir = snapshotsDir(this.store.workspaceDir(review.workspaceRoot));
        const files = new Map<string, string | undefined>();
        const read = async (uri: string) => {
            if (!files.has(uri)) {
                files.set(uri, await fs.readFile(FileUri.fsPath(uri), 'utf8').catch(() => undefined));
            }
            return files.get(uri);
        };
        const diffs = new Map<string, Edit[]>();
        const updates = new Map<string, CodeLocation>();
        for (const thread of review.threads) {
            const location = thread.location;
            const tracked = location.tracked;
            if (!tracked || !location.uri || thread.status === 'resolved') {
                continue;
            }
            const now = await read(location.uri);
            if (now === undefined) {
                // The file itself is gone.
                if (tracked.status !== 'removed') {
                    updates.set(thread.id, { ...location, tracked: { ...tracked, status: 'removed', candidates: undefined } });
                }
                continue;
            }
            const version = contentVersion(now);
            if (version === tracked.version) {
                continue;
            }
            // Co-Review's own edit, when the anchor is on the version it was made against; else a diff from the snapshot.
            const exact = explicit?.uri === location.uri && contentVersion(explicit.before) === tracked.version ? explicit : undefined;
            const before = exact ? exact.before : await fs.readFile(path.join(dir, tracked.version), 'utf8').catch(() => undefined);
            const key = `${location.uri}\n${tracked.version}\n${version}`;
            const edits = exact ? exact.edits : before !== undefined ? diffs.get(key) ?? editsBetween(before, now) : undefined;
            if (edits && !exact) {
                diffs.set(key, edits);
            }
            const next = track(tracked, before, now, version, edits);
            saveSnapshot(dir, version, now);
            updates.set(thread.id, this.follow(location, next, now));
        }
        if (updates.size) {
            await this.store.retrack(reviewId, updates);
            await this.collect(review.workspaceRoot);
        }
    }

    /** An edit Co-Review applied itself (an accepted suggestion): the file's anchors map through it exactly. */
    async applied(reviewId: string, uri: string, before: string, edits: Edit[]): Promise<void> {
        await this.running.get(reviewId);
        await this.doResolve(reviewId, { uri, edits, before }).catch(e => console.error('[co-review] tracking comments failed', e));
    }

    /** The location with its target where tracking found it: range and quote for code, quote and occurrence for pages. */
    protected follow(location: CodeLocation, tracked: TrackedRange, text: string): CodeLocation {
        const placed = tracked.status === 'active' || tracked.status === 'modified';
        if (!placed) {
            return { ...location, tracked };
        }
        if (location.kind === 'document' && location.docAnchor) {
            let occurrence = 0;
            for (let at = text.indexOf(tracked.text); at >= 0 && at < tracked.from; at = text.indexOf(tracked.text, at + 1)) {
                occurrence++;
            }
            return { ...location, tracked, docAnchor: { ...location.docAnchor, exact: tracked.text, occurrence } };
        }
        const start = positionAt(text, tracked.from);
        const end = positionAt(text, tracked.to);
        const lines = text.split('\n');
        return {
            ...location, tracked,
            kind: start.line === end.line ? (location.kind === 'symbol' ? 'symbol' : 'line') : (location.kind === 'symbol' ? 'symbol' : 'range'),
            range: { start, end },
            anchor: location.anchor && {
                ...location.anchor, text: tracked.text,
                before: start.line > 0 ? lines[start.line - 1] : undefined,
                after: end.line + 1 < lines.length ? lines[end.line + 1] : undefined,
                // The token anchor was the old text's; the tracked range is what locates it now.
                syntax: tracked.status === 'active' ? location.anchor.syntax : undefined
            }
        };
    }

    /** Deletes the workspace's snapshots that no comment of any of its reviews points to. */
    protected async collect(workspaceRoot: string): Promise<void> {
        const dir = snapshotsDir(this.store.workspaceDir(workspaceRoot));
        const used = new Set((await this.store.list(workspaceRoot)).flatMap((r: Review) => r.threads.map(t => t.location.tracked?.version).filter(Boolean)));
        for (const name of await fs.readdir(dir).catch(() => [] as string[])) {
            if (!used.has(name)) {
                await fs.rm(path.join(dir, name), { force: true });
            }
        }
    }
}
