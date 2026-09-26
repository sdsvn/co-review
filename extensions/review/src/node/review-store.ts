import { Emitter } from '@theia/core/lib/common/event';
import { FileUri } from '@theia/core/lib/common/file-uri';
import { injectable } from '@theia/core/shared/inversify';
import { createHash, randomUUID } from 'crypto';
import { promises as fs } from 'fs';
import * as os from 'os';
import * as path from 'path';
import { AgentConfig, AgentState, CodeLocation, Participant, Review, ReviewActivity, ReviewBundle, ReviewDecision, ReviewMessage, ReviewThread, ThreadOptions, ThreadStatus } from '../common/review-model';
import { CreateReviewParams } from '../common/review-protocol';

/** Where Co-Review keeps its state: `$CO_REVIEW_HOME`, default `~/.co-review`. */
export function coReviewHome(): string {
    return process.env.CO_REVIEW_HOME || path.join(os.homedir(), '.co-review');
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
        const hash = createHash('sha256').update(workspaceRoot).digest('hex').slice(0, 16);
        return path.join(coReviewHome(), 'workspaces', hash);
    }

    async list(workspaceRoot: string): Promise<Review[]> {
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
            this.record(review, { kind: 'thread-created', actor: author, threadId: thread.id, at: now });
            this.afterWrite(() => this.onDidAddMessageEmitter.fire({ review, thread, message: thread.messages[0] }));
            return thread;
        });
    }

    addMessage(reviewId: string, threadId: string, body: string, author: Participant): Promise<ReviewThread> {
        return this.mutateThread(reviewId, threadId, (review, thread, now) => {
            const message: ReviewMessage = { id: randomUUID(), author, body, createdAt: now };
            thread.messages.push(message);
            this.record(review, { kind: 'message-added', actor: author, threadId, at: now });
            this.afterWrite(() => this.onDidAddMessageEmitter.fire({ review, thread, message }));
        });
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
        return this.mutateThread(reviewId, threadId, (_review, thread) => {
            thread.intent = intent;
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
            review.verdict = { decision, summary, submittedAt: now, count: (review.verdict?.count ?? 0) + 1 };
            this.record(review, { kind: 'review-submitted', actor, at: now });
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
        return next;
    }

    protected async write(file: string, review: Review): Promise<void> {
        const tmp = `${file}.${process.pid}.tmp`;
        await fs.writeFile(tmp, JSON.stringify(review, undefined, 2), 'utf8');
        await fs.rename(tmp, file);
    }

    protected async writeWorkspaceInfo(workspaceRoot: string): Promise<void> {
        const info = { root: workspaceRoot, path: FileUri.fsPath(workspaceRoot) };
        await fs.writeFile(path.join(this.workspaceDir(workspaceRoot), 'workspace.json'), JSON.stringify(info, undefined, 2), 'utf8');
    }
}
