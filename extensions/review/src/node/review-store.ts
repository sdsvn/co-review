import { Emitter } from '@theia/core/lib/common/event';
import { FileUri } from '@theia/core/lib/common/file-uri';
import { injectable } from '@theia/core/shared/inversify';
import { createHash, randomUUID } from 'crypto';
import { promises as fs, readFileSync, realpathSync } from 'fs';
import * as os from 'os';
import * as path from 'path';
import { AgentConfig, AgentState, CodeLocation, Participant, Review, ReviewActivity, ReviewBundle, ReviewDecision, ReviewMessage, ReviewThread, ThreadOptions, ThreadStatus, contentVersion, isOverviewPage, waitsForAgent } from '../common/review-model';
import { CreateReviewParams } from '../common/review-protocol';

/** Where Co-Review keeps its state: `$CO_REVIEW_HOME`, default `~/.co-review`. */
export function coReviewHome(): string {
    return process.env.CO_REVIEW_HOME || path.join(os.homedir(), '.co-review');
}

/**
 * `p` resolved, with symbolic links followed (on macOS `/var` is `/private/var`, where `$TMPDIR` lives), as Theia
 * resolves the folder a window opens: the same folder always gets the same workspace key, whichever way it was
 * named. A path that doesn't exist yet keeps its missing part.
 */
export function realPath(p: string): string {
    const resolved = path.resolve(p);
    try {
        return realpathSync(resolved);
    } catch {
        const parent = path.dirname(resolved);
        return parent === resolved ? resolved : path.join(realPath(parent), path.basename(resolved));
    }
}

/** A `file:` URI with its path as {@link realPath}; other URIs as they are. */
export function realUri(uri: string): string {
    return uri.startsWith('file:') ? FileUri.create(realPath(FileUri.fsPath(uri))).toString() : uri;
}

export type ReviewChange = { kind: 'changed'; review: Review } | { kind: 'deleted'; reviewId: string; workspaceRoot: string };

/** A message was added to a thread (a new thread counts as its first message). */
export interface MessageAdded {
    review: Review;
    thread: ReviewThread;
    message: ReviewMessage;
}

/**
 * Local, file-based review persistence. Review state lives outside the repository:
 * `$CO_REVIEW_HOME` (default `~/.co-review`) / workspaces / <hash of root> / reviews / <id>.json
 */
@injectable()
export class ReviewStore {

    protected readonly onDidChangeEmitter = new Emitter<ReviewChange>();
    readonly onDidChange = this.onDidChangeEmitter.event;
    protected readonly onDidAddMessageEmitter = new Emitter<MessageAdded>();
    readonly onDidAddMessage = this.onDidAddMessageEmitter.event;

    /** reviewId -> file path, populated lazily as workspaces are listed. */
    protected readonly locations = new Map<string, string>();
    /** Serializes mutations per review so concurrent writers never interleave. */
    protected readonly queues = new Map<string, Promise<unknown>>();

    /** Where the reviews (and other state) of a workspace live, outside the repository. */
    workspaceDir(workspaceRoot: string): string {
        const hash = createHash('sha256').update(realUri(workspaceRoot)).digest('hex').slice(0, 16);
        return path.join(coReviewHome(), 'workspaces', hash);
    }

    /** Reviews saved before paths were resolved (see realPath), moved where their folder's windows look for them. */
    protected migration: Promise<void> | undefined;

    protected migrated(): Promise<void> {
        return this.migration ??= this.migrate().catch(e => console.error('[co-review] moving reviews to resolved paths failed', e));
    }

    protected async migrate(): Promise<void> {
        const workspaces = path.join(coReviewHome(), 'workspaces');
        for (const ws of await fs.readdir(workspaces).catch(() => [] as string[])) {
            const info = await fs.readFile(path.join(workspaces, ws, 'workspace.json'), 'utf8').then(t => JSON.parse(t) as { root: string }, () => undefined);
            const real = info && realUri(info.root);
            if (!info || !real || real === info.root) {
                continue;
            }
            const from = FileUri.fsPath(info.root);
            const to = FileUri.fsPath(real);
            const fromUri = info.root;
            // The paths and URIs under the old name, renamed: the workspace root, the review directory and its files,
            // thread locations and scopes; never message text, which may quote a path.
            const PATHS = new Set(['workspaceRoot', 'dir', 'storePath', 'openspec', 'uri', 'uris']);
            const rename = (value: unknown, key?: string): unknown => {
                if (typeof value === 'string') {
                    if (!key || !PATHS.has(key)) {
                        return value;
                    }
                    for (const [a, b] of [[fromUri, real], [from, to]]) {
                        if (value === a || value.startsWith(a + '/')) {
                            return b + value.slice(a.length);
                        }
                    }
                    return value;
                }
                if (Array.isArray(value)) {
                    return value.map(v => rename(v, key));
                }
                if (value && typeof value === 'object') {
                    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, rename(v, k)]));
                }
                return value;
            };
            const source = path.join(workspaces, ws, 'reviews');
            const target = path.join(this.workspaceDir(real), 'reviews');
            await fs.mkdir(target, { recursive: true });
            await this.writeWorkspaceInfo(real);
            for (const name of (await fs.readdir(source).catch(() => [] as string[])).filter(n => n.endsWith('.json'))) {
                const review = rename(JSON.parse(await fs.readFile(path.join(source, name), 'utf8')));
                await fs.writeFile(path.join(target, name), JSON.stringify(review, undefined, 2), 'utf8');
            }
            // The rest (repository map, pages) is rebuilt under the new key.
            await fs.rm(path.join(workspaces, ws), { recursive: true, force: true });
            console.info(`[co-review] reviews of ${from} moved to ${to}`);
        }
    }

    async list(workspaceRoot: string): Promise<Review[]> {
        await this.migrated();
        const dir = path.join(this.workspaceDir(workspaceRoot), 'reviews');
        let names: string[];
        try {
            names = await fs.readdir(dir);
        } catch {
            return [];
        }
        const reviews: Review[] = [];
        for (const name of names.filter(n => n.endsWith('.json'))) {
            const file = path.join(dir, name);
            try {
                const review = JSON.parse(await fs.readFile(file, 'utf8')) as Review;
                this.locations.set(review.id, file);
                reviews.push(review);
            } catch (e) {
                console.error(`[co-review] failed to read ${file}`, e);
            }
        }
        return reviews.sort((a, b) => b.updatedAt - a.updatedAt);
    }

    /** Finds the file of a review, also after a backend restart when no workspace was listed yet. */
    protected async locate(reviewId: string): Promise<string | undefined> {
        await this.migrated();
        const known = this.locations.get(reviewId);
        if (known) {
            return known;
        }
        const workspaces = path.join(coReviewHome(), 'workspaces');
        for (const ws of await fs.readdir(workspaces).catch(() => [] as string[])) {
            const file = path.join(workspaces, ws, 'reviews', `${reviewId}.json`);
            if (await fs.stat(file).then(() => true, () => false)) {
                this.locations.set(reviewId, file);
                return file;
            }
        }
        return undefined;
    }

    /**
     * The reviews a window of `workspaceRoot` shows: the workspace's own, and change reviews whose code folder it is
     * (their review directory is elsewhere; see CodeFolders).
     */
    async listForWindow(workspaceRoot: string): Promise<Review[]> {
        const root = realUri(workspaceRoot);
        const own = await this.list(root);
        const coded = (await this.listAll()).filter(r => r.bundle?.code && FileUri.create(r.bundle.code).toString() === root && !own.some(o => o.id === r.id));
        return [...own, ...coded].sort((a, b) => b.updatedAt - a.updatedAt);
    }

    /** Every review in every workspace (for the mobile view). */
    async listAll(): Promise<Review[]> {
        const workspaces = path.join(coReviewHome(), 'workspaces');
        const all: Review[] = [];
        for (const ws of await fs.readdir(workspaces).catch(() => [] as string[])) {
            try {
                const info = JSON.parse(await fs.readFile(path.join(workspaces, ws, 'workspace.json'), 'utf8'));
                all.push(...await this.list(info.root));
            } catch {
                /* not a workspace folder */
            }
        }
        return all.sort((a, b) => b.updatedAt - a.updatedAt);
    }

    async get(reviewId: string): Promise<Review | undefined> {
        const file = await this.locate(reviewId);
        if (!file) {
            return undefined;
        }
        try {
            return JSON.parse(await fs.readFile(file, 'utf8'));
        } catch {
            return undefined;
        }
    }

    async create(params: CreateReviewParams): Promise<Review> {
        await this.migrated();
        params = { ...params, workspaceRoot: realUri(params.workspaceRoot) };
        const now = Date.now();
        const review: Review = {
            id: randomUUID(),
            title: params.title,
            workspaceRoot: params.workspaceRoot,
            scope: params.scope,
            participants: [params.author],
            threads: [],
            activity: [{ kind: 'review-created', actor: params.author, at: now }],
            nextThreadNumber: 1,
            createdAt: now,
            updatedAt: now
        };
        const dir = path.join(this.workspaceDir(params.workspaceRoot), 'reviews');
        await fs.mkdir(dir, { recursive: true });
        await this.writeWorkspaceInfo(params.workspaceRoot);
        const file = path.join(dir, `${review.id}.json`);
        this.locations.set(review.id, file);
        await this.write(file, review);
        this.onDidChangeEmitter.fire({ kind: 'changed', review });
        return review;
    }

    async delete(reviewId: string): Promise<void> {
        const review = await this.get(reviewId);
        const file = await this.locate(reviewId);
        if (!review || !file) {
            return;
        }
        await fs.rm(file, { force: true });
        this.locations.delete(reviewId);
        this.onDidChangeEmitter.fire({ kind: 'deleted', reviewId, workspaceRoot: review.workspaceRoot });
    }

    rename(reviewId: string, title: string): Promise<Review> {
        return this.mutate(reviewId, review => {
            review.title = title;
            return review;
        });
    }

    archive(reviewId: string, archived: boolean): Promise<Review> {
        return this.mutate(reviewId, review => {
            review.archivedAt = archived ? Date.now() : undefined;
            return review;
        });
    }

    createThread(reviewId: string, location: CodeLocation, body: string, author: Participant, options: ThreadOptions = {}): Promise<ReviewThread> {
        location = this.versioned(location);
        return this.mutate(reviewId, (review, now) => {
            const thread: ReviewThread = {
                id: randomUUID(),
                number: review.nextThreadNumber++,
                location,
                messages: [{ id: randomUUID(), author, body, createdAt: now }],
                status: options.status ?? 'open',
                intent: options.intent ?? 'comment',
                severity: options.severity,
                origin: options.origin,
                sourceId: options.sourceId,
                labels: options.labels,
                proposal: options.proposal,
                createdAt: now,
                updatedAt: now
            };
            review.threads.push(thread);
            this.queueForAgent(review, thread);
            this.record(review, { kind: 'thread-created', actor: author, threadId: thread.id, at: now });
            this.afterWrite(() => this.onDidAddMessageEmitter.fire({ review, thread, message: thread.messages[0] }));
            return thread;
        });
    }

    addMessage(reviewId: string, threadId: string, body: string, author: Participant, extra?: Pick<ReviewMessage, 'choice'>): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (review, thread, now) => {
            const message: ReviewMessage = { ...extra, id: randomUUID(), author, body, createdAt: now };
            thread.messages.push(message);
            this.queueForAgent(review, thread);
            this.record(review, { kind: 'message-added', actor: author, threadId, at: now });
            this.afterWrite(() => this.onDidAddMessageEmitter.fire({ review, thread, message }));
        });
    }

    /**
     * A comment on a document is on the version of it that is there now (see DocAnchor.version), whoever makes it (the
     * reviewer, an agent's finding): stamped here, where every thread is created.
     */
    protected versioned(location: CodeLocation): CodeLocation {
        const anchor = location.docAnchor;
        if (location.kind !== 'document' || !anchor || anchor.version || !location.uri?.startsWith('file:') || isOverviewPage(location.uri)) {
            return location;
        }
        try {
            return { ...location, docAnchor: { ...anchor, version: contentVersion(readFileSync(FileUri.fsPath(location.uri), 'utf8')) } };
        } catch {
            return location;
        }
    }

    /** The reviewer's latest message is for the review's MCP agent: it waits for the agent to pick it up (shown as such). */
    protected queueForAgent(review: Review, thread: ReviewThread): void {
        if (review.agent?.transport === 'mcp' && waitsForAgent(review, thread, review.agent.id)) {
            thread.agentState = 'queued';
        }
    }

    /** Adds an (agent) message that is filled in later with {@link updateMessage}; returns its id. */
    async startMessage(reviewId: string, threadId: string, author: Participant, init: Partial<ReviewMessage>): Promise<string> {
        const id = randomUUID();
        await this.mutateThread(reviewId, threadId, (review, thread, now) => {
            thread.messages.push({ body: '', ...init, id, author, createdAt: now });
            this.record(review, { kind: 'message-added', actor: author, threadId, at: now });
        });
        return id;
    }

    updateMessage(reviewId: string, threadId: string, messageId: string, patch: Partial<Omit<ReviewMessage, 'id' | 'author'>>): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (_review, thread) => {
            const message = thread.messages.find(m => m.id === messageId);
            if (message) {
                Object.assign(message, patch);
            }
        });
    }

    /** Records the decision on a suggestion and resolves the thread, with a note from the system. */
    decideProposal(reviewId: string, threadId: string, accepted: boolean, note: string, docVersion?: number): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (review, thread, now) => {
            if (!thread.proposal) {
                return;
            }
            thread.proposal.status = accepted ? 'accepted' : 'rejected';
            thread.status = 'resolved';
            thread.messages.push({ id: randomUUID(), author: { id: 'system', kind: 'system', name: 'system' }, body: note, createdAt: now });
            if (docVersion !== undefined && review.bundle) {
                review.bundle.docVersion = docVersion;
                // The document now holds the replacement: anchor the thread to it.
                const docAnchor = thread.location.docAnchor;
                if (docAnchor?.type === 'text') {
                    thread.location = { ...thread.location, docAnchor: { ...docAnchor, exact: thread.proposal.after }, anchor: { text: thread.proposal.after } };
                }
            }
        });
    }

    setAgentState(reviewId: string, threadId: string, state: AgentState): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (_review, thread) => {
            thread.agentState = state;
        });
    }

    setThreadIntent(reviewId: string, threadId: string, intent: ReviewThread['intent']): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (review, thread) => {
            thread.intent = intent;
            this.queueForAgent(review, thread);
        });
    }

    setBundle(reviewId: string, bundle: ReviewBundle | undefined): Promise<Review> {
        return this.mutate(reviewId, review => {
            review.bundle = bundle;
            return review;
        });
    }

    /** The reviewer submits the review (a round): decision + message to the agent. */
    submit(reviewId: string, decision: ReviewDecision, summary: string, actor: Participant): Promise<Review> {
        return this.mutate(reviewId, (review, now) => {
            review.verdict = {
                decision, summary, submittedAt: now, count: (review.verdict?.count ?? 0) + 1,
                ...review.agent?.transport === 'mcp' ? { toAgent: 'waiting' as const } : {}
            };
            this.record(review, { kind: 'review-submitted', actor, at: now });
            return review;
        });
    }

    setOverview(reviewId: string, overview: string): Promise<Review> {
        return this.mutate(reviewId, review => {
            review.overview = overview;
            return review;
        });
    }

    /** The reviewer closed the review's window (`at`), or it is shown again (undefined). */
    setClosed(reviewId: string, closed: Review['closed']): Promise<Review> {
        return this.mutate(reviewId, review => {
            review.closed = closed;
            return review;
        });
    }

    /** An agent picked up the latest round. */
    verdictDelivered(reviewId: string): Promise<Review> {
        return this.mutate(reviewId, review => {
            if (review.verdict) {
                review.verdict.toAgent = 'delivered';
            }
            return review;
        });
    }

    /** Marks repository-relative files as viewed (or not). */
    setViewed(reviewId: string, paths: string[], viewed: boolean): Promise<Review> {
        return this.mutate(reviewId, (review, now) => {
            const map = review.viewed ??= {};
            for (const p of paths) {
                if (viewed) {
                    map[p] = now;
                } else {
                    delete map[p];
                }
            }
            return review;
        });
    }

    setAgent(reviewId: string, agent: AgentConfig | undefined): Promise<Review> {
        return this.mutate(reviewId, review => {
            review.agent = agent;
            return review;
        });
    }

    setThreadStatus(reviewId: string, threadId: string, status: ThreadStatus, actor: Participant): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (review, thread, now) => {
            if (thread.status !== status) {
                thread.status = status;
                this.record(review, { kind: status === 'resolved' ? 'thread-resolved' : 'thread-reopened', actor, threadId, at: now });
            }
        });
    }

    relocateThread(reviewId: string, threadId: string, location: CodeLocation, actor: Participant): Promise<ReviewThread> {
        // Relocation is automatic bookkeeping, not review activity.
        return this.mutateThread(reviewId, threadId, (_review, thread) => {
            thread.location = location;
        });
    }

    protected record(review: Review, activity: ReviewActivity): void {
        review.activity.push(activity);
        if (!review.participants.some(p => p.id === activity.actor.id)) {
            review.participants.push(activity.actor);
        }
    }

    protected mutateThread(reviewId: string, threadId: string,
        fn: (review: Review, thread: ReviewThread, now: number) => void): Promise<ReviewThread> {
        return this.mutate(reviewId, (review, now) => {
            const thread = review.threads.find(t => t.id === threadId);
            if (!thread) {
                throw new Error(`Unknown thread ${threadId} in review ${reviewId}`);
            }
            fn(review, thread, now);
            thread.updatedAt = now;
            return thread;
        });
    }

    /** Callbacks to run once the current mutation has been written and broadcast. */
    protected pendingAfterWrite: (() => void)[] = [];

    protected afterWrite(fn: () => void): void {
        this.pendingAfterWrite.push(fn);
    }

    protected mutate<T>(reviewId: string, fn: (review: Review, now: number) => T): Promise<T> {
        const previous = this.queues.get(reviewId) ?? Promise.resolve();
        const next = previous.catch(() => undefined).then(async () => {
            const file = await this.locate(reviewId);
            const review = await this.get(reviewId);
            if (!file || !review) {
                throw new Error(`Unknown review ${reviewId}`);
            }
            const now = Date.now();
            const result = fn(review, now);
            review.updatedAt = now;
            const after = this.pendingAfterWrite.splice(0);
            await this.write(file, review);
            this.onDidChangeEmitter.fire({ kind: 'changed', review });
            after.forEach(f => f());
            return result;
        });
        this.queues.set(reviewId, next);
        const forget = () => this.queues.get(reviewId) === next && this.queues.delete(reviewId);
        next.then(forget, forget);
        return next;
    }

    protected async write(file: string, review: Review): Promise<void> {
        const tmp = `${file}.${process.pid}.tmp`;
        await fs.writeFile(tmp, JSON.stringify(review, undefined, 2), 'utf8');
        await fs.rename(tmp, file);
    }

    protected async writeWorkspaceInfo(workspaceRoot: string): Promise<void> {
        const root = realUri(workspaceRoot);
        const info = { root, path: FileUri.fsPath(root) };
        await fs.writeFile(path.join(this.workspaceDir(workspaceRoot), 'workspace.json'), JSON.stringify(info, undefined, 2), 'utf8');
    }
}
