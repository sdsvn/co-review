import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { execFile } from 'child_process';
import { createHash } from 'crypto';
import { existsSync, promises as fs } from 'fs';
import * as path from 'path';
import { FileUri } from '@theia/core/lib/common/file-uri';
import { CodeLocation, PatchAnchor, Review, ReviewBundle } from '../common/review-model';
import { parsePatch } from '../common/patch';
import { BundleService } from './bundle-service';
import { coReviewHome, realPath, ReviewStore } from './review-store';

/** `git -C cwd …`; `out` trimmed unless `raw` (file contents). */
function git(cwd: string, args: string[], raw = false): Promise<{ ok: boolean; out: string; err: string }> {
    return new Promise(resolve => execFile('git', ['-C', cwd, ...args], { maxBuffer: 64 * 1024 * 1024 },
        (error, stdout, stderr) => resolve({ ok: !error, out: raw ? String(stdout) : String(stdout).trim(), err: String(stderr).trim() })));
}

/** The two sides of a revision range as `git diff` reads it; `head` undefined: the working tree. */
export function sidesOf(range: string): { base: string; head?: string; mergeBase: boolean } {
    const three = range.split('...');
    if (three.length === 2) {
        return { base: three[0] || 'HEAD', head: three[1] || 'HEAD', mergeBase: true };
    }
    const two = range.split('..');
    if (two.length === 2) {
        return { base: two[0] || 'HEAD', head: two[1] || 'HEAD', mergeBase: false };
    }
    return { base: range, head: undefined, mergeBase: false };
}

/**
 * The code a change review shows around its diff, so the IDE (explorer, search, go to definition, hover) works on
 * real files: for the reviewer's own change the repository itself; for a pull request (a head that isn't what the
 * repository has checked out) a git worktree of its head under `~/.co-review/worktrees/`, which shares the
 * repository's objects. The worktree checks out the change's neighbourhood (the changed and related folders) first,
 * so the code is there in seconds, and the rest in the background. It is removed as soon as the review is archived
 * or deleted, and created again when the review is unarchived. The review stays keyed by its review directory;
 * `bundle.code` is where its window opens.
 */
@injectable()
export class CodeFolders {

    @inject(ReviewStore) protected readonly store: ReviewStore;
    @inject(BundleService) protected readonly bundles: BundleService;

    /** Worktrees by review, so a deleted review's worktree can be removed (a deletion carries no review). */
    protected readonly worktrees = new Map<string, { repo: string; code: string }>();
    /** Reviews whose worktree is being created, so changes arriving meanwhile don't start a second one. */
    protected readonly creating = new Set<string>();

    @postConstruct()
    protected init(): void {
        this.store.onDidChange(change => {
            if (change.kind === 'deleted') {
                const known = this.worktrees.get(change.reviewId);
                this.worktrees.delete(change.reviewId);
                if (known) {
                    this.remove(known.repo, known.code).catch(e => console.error('[co-review] removing a worktree failed', e));
                }
                return;
            }
            const bundle = change.review.bundle;
            if (!bundle?.worktree || !bundle.repo || !bundle.code) {
                return;
            }
            this.worktrees.set(change.review.id, { repo: bundle.repo, code: bundle.code });
            if (change.review.archivedAt && existsSync(bundle.code)) {
                this.remove(bundle.repo, bundle.code).catch(e => console.error('[co-review] removing a worktree failed', e));
            } else if (!change.review.archivedAt && !existsSync(bundle.code) && bundle.head && !this.creating.has(change.review.id)) {
                // Unarchived: the worktree again, at the same head and base.
                this.checkout(change.review, bundle.repo, bundle.head, [], bundle.base).catch(e => console.error('[co-review] recreating a worktree failed', e));
            }
        });
    }

    /**
     * Gives a change review (the `range` diffed in `repo`) its code folder and records it on the review. `changed`
     * and `related` are repository-relative files: their folders are checked out first. Returns the code folder.
     */
    async prepare(review: Review, repo: string, range: string, changed: string[], related: string[] = []): Promise<string> {
        const sides = sidesOf(range);
        const sha = async (rev: string) => (await git(repo, ['rev-parse', '--verify', `${rev}^{commit}`])).out || undefined;
        const head = sides.head ? await sha(sides.head) : undefined;
        const current = await sha('HEAD');
        let base = await sha(sides.base);
        if (sides.mergeBase && base && head) {
            base = (await git(repo, ['merge-base', base, head])).out || base;
        }
        // The reviewer's own change (the working tree, or what the repository has checked out): the repository itself.
        if (!head || head === current) {
            await this.record(review, { repo, code: realPath(repo), base, head: undefined, worktree: false, checkout: 'full', related });
            return realPath(repo);
        }
        const code = await this.checkout(review, repo, head, [...changed, ...related], base);
        await this.record(review, { related });
        return code;
    }

    /** Where a review's worktree lives: per repository and review, outside the repository. */
    protected folderOf(repo: string, reviewId: string): string {
        const hash = createHash('sha256').update(realPath(repo)).digest('hex').slice(0, 8);
        return path.join(coReviewHome(), 'worktrees', `${path.basename(repo)}-${hash}`, reviewId.slice(0, 8));
    }

    protected async checkout(review: Review, repo: string, head: string, files: string[], base?: string): Promise<string> {
        this.creating.add(review.id);
        try {
            return await this.doCheckout(review, repo, head, files, base);
        } finally {
            this.creating.delete(review.id);
        }
    }

    protected async doCheckout(review: Review, repo: string, head: string, files: string[], base?: string): Promise<string> {
        const code = this.folderOf(repo, review.id);
        if (existsSync(path.join(code, '.git'))) {
            // Opened again (new commits, or the same): move to the head; a full checkout stays full.
            const moved = await git(code, ['checkout', '--quiet', '--detach', '--force', head]);
            if (!moved.ok) {
                throw new Error(`git checkout ${head} in ${code} failed: ${moved.err}`);
            }
            await this.record(review, { repo, code: realPath(code), base, head, worktree: true, checkout: review.bundle?.checkout ?? 'full' });
            this.fill(review.id, code);
            return realPath(code);
        }
        await fs.mkdir(path.dirname(code), { recursive: true });
        await git(repo, ['worktree', 'prune']);
        const added = await git(repo, ['worktree', 'add', '--quiet', '--no-checkout', '--detach', code, head]);
        if (!added.ok) {
            throw new Error(`git worktree add failed: ${added.err}`);
        }
        // The neighbourhood first: the folders of the changed and related files (cone mode also has the top-level files).
        const folders = [...new Set(files.map(f => path.posix.dirname(f)).filter(d => d && d !== '.'))];
        if (folders.length) {
            await git(code, ['sparse-checkout', 'set', '--cone', ...folders]);
        }
        const checked = await git(code, ['checkout', '--quiet', '--detach', head]);
        if (!checked.ok) {
            throw new Error(`git checkout ${head} failed: ${checked.err}`);
        }
        await this.record(review, { repo, code: realPath(code), base, head, worktree: true, checkout: folders.length ? 'neighbourhood' : 'full' });
        if (folders.length) {
            this.fill(review.id, code);
        }
        return realPath(code);
    }

    /** The rest of the code, in the background, once the neighbourhood is there. */
    protected fill(reviewId: string, code: string): void {
        (async () => {
            const sparse = await git(code, ['config', '--get', 'core.sparseCheckout']);
            if (sparse.out === 'true') {
                const done = await git(code, ['sparse-checkout', 'disable']);
                if (!done.ok) {
                    throw new Error(done.err);
                }
            }
            const review = await this.store.get(reviewId);
            if (review?.bundle && review.bundle.checkout !== 'full') {
                await this.store.setBundle(review.id, { ...review.bundle, checkout: 'full' });
            }
        })().catch(e => console.error(`[co-review] checking out the rest of ${code} failed`, e));
    }

    protected async record(review: Review, code: Partial<Pick<ReviewBundle, 'repo' | 'code' | 'base' | 'head' | 'worktree' | 'checkout' | 'related'>>): Promise<void> {
        const latest = await this.store.get(review.id);
        if (latest?.bundle) {
            await this.store.setBundle(review.id, { ...latest.bundle, ...code });
        }
    }

    protected async remove(repo: string, code: string): Promise<void> {
        if (!existsSync(code)) {
            return;
        }
        const removed = await git(repo, ['worktree', 'remove', '--force', code]);
        if (!removed.ok) {
            await fs.rm(code, { recursive: true, force: true });
            await git(repo, ['worktree', 'prune']);
        }
    }

    /**
     * A comment written in the editor on lines the diff shows (added or context) becomes a comment on the diff, so it
     * is on the diff page and goes to GitHub like one made there; anywhere else it stays a comment on the file.
     * Only when the lines read as they do in the diff: a file edited since the diff isn't mapped onto it.
     */
    async onDiff(review: Review, location: CodeLocation): Promise<CodeLocation | undefined> {
        const code = review.bundle?.code;
        if (!code || !review.bundle || !location.range || !location.uri?.startsWith('file:')) {
            return undefined;
        }
        const file = path.relative(code, FileUri.fsPath(location.uri)).split(path.sep).join('/');
        if (file.startsWith('..')) {
            return undefined;
        }
        const start = location.range.start.line + 1;
        const end = location.range.end.line + 1;
        const text = location.anchor?.text?.split('\n');
        for (const patchFile of this.bundles.patchesOf(review.bundle.dir)) {
            const shown = parsePatch(await fs.readFile(patchFile, 'utf8').catch(() => ''))
                .find(f => f.path === file)?.rows.filter(r => r.t !== 'hunk' && r.t !== 'del' && r.n !== undefined);
            const rows = shown && Array.from({ length: end - start + 1 }, (_, i) => shown.find(r => r.n === start + i));
            if (!rows || rows.some(r => !r) || (text && rows.some((r, i) => text[i] !== undefined && r!.s !== text[i]))) {
                continue;
            }
            const anchor: PatchAnchor = start === end
                ? { type: 'code-line', path: file, line: start, side: 'new', source: location.anchor?.text }
                : { type: 'code-range', path: file, startLine: start, endLine: end, side: 'new', source: location.anchor?.text };
            return { kind: 'patch', uri: FileUri.create(patchFile).toString(), patchAnchor: anchor, anchor: { text: location.anchor?.text ?? '' } };
        }
        return undefined;
    }

    /** The base version of a repository-relative file (for the old side and Compare); undefined when it didn't exist. */
    async baseFile(review: Review, file: string): Promise<string | undefined> {
        const bundle = review.bundle;
        if (!bundle?.repo || !bundle.base || file.startsWith('/') || file.split('/').includes('..')) {
            return undefined;
        }
        const shown = await git(bundle.repo, ['show', `${bundle.base}:${file}`], true);
        return shown.ok ? shown.out : undefined;
    }
}
