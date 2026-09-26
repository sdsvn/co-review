import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { createHash } from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import { Participant, Review, Severity } from '../common/review-model';
import { DOCUMENT_NAMES, parsePatchMeta } from '../common/patch';
import { locationFromAnchor, toStateFile } from './review-payloads';
import { coReviewHome, ReviewStore } from './review-store';

export interface OpenBundleParams {
    dir?: string;
    markdown?: string;
    patch?: string;
    patchName?: string;
    title?: string;
    storePath?: string;
    /** OpenSpec change directory (…/changes/<id>); `<dir>/openspec` is picked up automatically. */
    openspec?: string;
    author: Participant;
}

export interface OpenedBundle {
    review: Review;
    dir: string;
    docFile?: string;
    patches: string[];
}

interface Finding {
    id: string;
    proposal?: { before: string; after: string; path?: string; startLine?: number };
    target?: string;
    anchor: Record<string, any>;
    severity?: Severity;
    labels?: string[];
    body: string;
    verdict?: string;
}

/**
 * Review directories: a document (`index.markdown`, `*.pseudocode.md`, …),
 * `*.patch` / `*.diff` pages, `PR.md` metadata and `<patch>.comments.json[l]` pre-seeded findings.
 * The review state is also written to the directory as `review.json`.
 */
@injectable()
export class BundleService {

    @inject(ReviewStore) protected readonly store: ReviewStore;

    protected readonly watchers = new Map<string, fs.FSWatcher>();
    protected readonly exportTimers = new Map<string, NodeJS.Timeout>();

    @postConstruct()
    protected init(): void {
        this.store.onDidChange(change => {
            if (change.kind === 'changed' && change.review.bundle) {
                this.scheduleExport(change.review);
            } else if (change.kind === 'deleted') {
                // A review directory's review is its workspace root.
                const dir = FileUri.fsPath(change.workspaceRoot);
                this.watchers.get(dir)?.close();
                this.watchers.delete(dir);
                clearTimeout(this.exportTimers.get(change.reviewId));
                this.exportTimers.delete(change.reviewId);
            }
        });
    }

    /** Opens (or re-opens) the review of a review directory; inline content is written to a directory first. */
    async open(params: OpenBundleParams): Promise<OpenedBundle> {
        const dir = params.dir ? path.resolve(params.dir) : this.writeInline(params);
        const workspaceRoot = FileUri.create(dir).toString();
        const docFile = this.documentOf(dir);
        const title = params.title ?? (docFile ? this.titleOf(docFile) : path.basename(dir));
        let review = (await this.store.list(workspaceRoot))[0]
            ?? await this.store.create({ workspaceRoot, title, scope: { kind: 'repository' }, author: params.author });
        const detected = fs.existsSync(path.join(dir, 'openspec', 'proposal.md')) ? path.join(dir, 'openspec') : undefined;
        const openspec = params.openspec ? path.resolve(params.openspec) : review.bundle?.openspec ?? detected;
        const bundle = { ...review.bundle, dir, storePath: params.storePath ? path.resolve(params.storePath) : review.bundle?.storePath, openspec };
        review = await this.store.setBundle(review.id, bundle);
        await this.syncFindings(review.id);
        this.watch(review.id, dir);
        return { review: (await this.store.get(review.id))!, dir, docFile, patches: this.patchesOf(dir) };
    }

    protected writeInline(params: OpenBundleParams): string {
        const hash = createHash('sha256').update((params.markdown ?? '') + '\0' + (params.patch ?? '')).digest('hex').slice(0, 12);
        const dir = path.join(coReviewHome(), 'inline', hash);
        fs.mkdirSync(dir, { recursive: true });
        if (params.markdown) {
            const md = params.markdown.match(/^#\s+/m) || !params.title ? params.markdown : `# ${params.title}\n\n${params.markdown}`;
            fs.writeFileSync(path.join(dir, 'index.markdown'), md);
        }
        if (params.patch) {
            fs.writeFileSync(path.join(dir, `${params.patchName || 'change'}.patch`), params.patch);
        }
        return dir;
    }

    documentOf(dir: string): string | undefined {
        for (const name of DOCUMENT_NAMES) {
            if (fs.existsSync(path.join(dir, name))) {
                return path.join(dir, name);
            }
        }
        const pseudo = fs.readdirSync(dir).filter(f => f.endsWith('.pseudocode.md')).sort()[0];
        return pseudo ? path.join(dir, pseudo) : undefined;
    }

    patchesOf(dir: string): string[] {
        return fs.readdirSync(dir).filter(f => /\.(patch|diff)$/.test(f)).sort().map(f => path.join(dir, f));
    }

    protected titleOf(file: string): string {
        const h1 = fs.readFileSync(file, 'utf8').match(/^#\s+(.+?)\s*$/m);
        return h1 ? h1[1].replace(/^Feature:\s*/i, '') : path.basename(path.dirname(file));
    }

    /**
     * Accepting a patch suggestion records it (the agent applies it as a commit; the reviewed patch
     * stays as it is); accepting a document suggestion writes it into the document.
     */
    async decideProposal(reviewId: string, threadId: string, accept: boolean): Promise<void> {
        const review = await this.store.get(reviewId);
        const thread = review?.threads.find(t => t.id === threadId);
        const proposal = thread?.proposal;
        if (!review || !thread || !proposal || proposal.status !== 'pending') {
            return;
        }
        if (!accept) {
            await this.store.decideProposal(reviewId, threadId, false, 'Edit rejected.');
            return;
        }
        if (thread.location.kind === 'patch' && thread.location.uri) {
            const patch = FileUri.fsPath(thread.location.uri);
            const slug = path.basename(patch).replace(/\.(patch|diff)$/, '');
            const metaFile = [path.join(path.dirname(patch), `${slug}.md`), path.join(path.dirname(patch), 'PR.md')].find(f => fs.existsSync(f));
            const branch = metaFile ? parsePatchMeta(fs.readFileSync(metaFile, 'utf8')).branch : undefined;
            await this.store.decideProposal(reviewId, threadId, true,
                `Suggestion accepted. Apply it as a new commit on ${branch ?? 'the branch'}; the reviewed patch stays as it is.`);
            return;
        }
        const file = thread.location.uri ? FileUri.fsPath(thread.location.uri) : undefined;
        const text = file && fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : undefined;
        if (file && text !== undefined && text.includes(proposal.before)) {
            fs.writeFileSync(file, text.replace(proposal.before, proposal.after));
            const version = (review.bundle?.docVersion ?? 1) + 1;
            await this.store.decideProposal(reviewId, threadId, true, `Edit accepted. Document version ${version} created.`, version);
        } else {
            await this.store.decideProposal(reviewId, threadId, true, 'Accepted, but the quoted text was no longer in the source, so no new version was written.');
        }
    }

    /** Pre-seeded findings from `<patch>.comments.json` / `.jsonl`, as proposed threads; idempotent by finding id. */
    async syncFindings(reviewId: string): Promise<number> {
        const review = await this.store.get(reviewId);
        const dir = review?.bundle?.dir;
        if (!review || !dir || !fs.existsSync(dir)) {
            return 0;
        }
        const seen = new Set(review.threads.map(t => t.sourceId).filter(Boolean));
        const docFile = this.documentOf(dir);
        let created = 0;
        for (const patch of this.patchesOf(dir)) {
            const slug = path.basename(patch).replace(/\.(patch|diff)$/, '');
            for (const finding of this.readFindings(dir, slug)) {
                if (!finding.id || seen.has(finding.id)) {
                    continue;
                }
                seen.add(finding.id);
                await this.addFinding(review.id, dir, docFile, { ...finding, target: `patch:${slug}` });
                created++;
            }
        }
        return created;
    }

    async addFinding(reviewId: string, dir: string, docFile: string | undefined, finding: Finding, author?: Participant): Promise<void> {
        const location = locationFromAnchor(dir, docFile, finding.target, finding.anchor ?? {});
        const body = finding.verdict ? `${finding.body}\n\n_${finding.verdict}_` : finding.body;
        await this.store.createThread(reviewId, location, body, author ?? { id: 'agent:review-bot', kind: 'agent', name: 'review-bot' }, {
            status: 'proposed', origin: 'finding', sourceId: finding.id, severity: finding.severity, labels: finding.labels,
            proposal: finding.proposal && { ...finding.proposal, status: 'pending' }
        });
    }

    protected readFindings(dir: string, slug: string): Finding[] {
        for (const ext of ['.comments.json', '.comments.jsonl']) {
            const file = path.join(dir, slug + ext);
            if (!fs.existsSync(file)) {
                continue;
            }
            try {
                const text = fs.readFileSync(file, 'utf8');
                return ext.endsWith('l')
                    ? text.split('\n').filter(l => l.trim()).map(l => JSON.parse(l))
                    : JSON.parse(text);
            } catch (e) {
                console.error(`[co-review] cannot read findings ${file}`, e);
            }
            return [];
        }
        return [];
    }

    /** New or edited sidecar files are ingested while the review is open. */
    protected watch(reviewId: string, dir: string): void {
        if (this.watchers.has(dir)) {
            return;
        }
        try {
            let timer: NodeJS.Timeout | undefined;
            const watcher = fs.watch(dir, (_event, file) => {
                if (file && /\.comments\.jsonl?$/.test(String(file))) {
                    clearTimeout(timer);
                    timer = setTimeout(() => this.syncFindings(reviewId).catch(() => undefined), 300);
                }
            });
            this.watchers.set(dir, watcher);
        } catch {
            /* directory not watchable */
        }
    }

    protected scheduleExport(review: Review): void {
        clearTimeout(this.exportTimers.get(review.id));
        this.exportTimers.set(review.id, setTimeout(() => this.export(review.id).catch(e => console.error('[co-review] review.json export failed', e)), 150));
    }

    /** Writes the review's `review.json` (or the configured store path). */
    async export(reviewId: string): Promise<void> {
        const review = await this.store.get(reviewId);
        const bundle = review?.bundle;
        if (!review || !bundle || !fs.existsSync(bundle.dir)) {
            return;
        }
        const docFile = this.documentOf(bundle.dir);
        const doc = docFile ? { slug: path.basename(docFile), title: this.titleOf(docFile), markdown: fs.readFileSync(docFile, 'utf8') } : undefined;
        const file = bundle.storePath ?? path.join(bundle.dir, 'review.json');
        fs.mkdirSync(path.dirname(file), { recursive: true });
        const tmp = `${file}.${process.pid}.tmp`;
        fs.writeFileSync(tmp, JSON.stringify(toStateFile(review, doc), undefined, 2));
        fs.renameSync(tmp, file);
    }
}
