import { Emitter } from '@theia/core/lib/common/event';
import URI from '@theia/core/lib/common/uri';
import { StorageService } from '@theia/core/lib/browser/storage-service';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { WorkspaceService } from '@theia/workspace/lib/browser/workspace-service';
import { AgentConfig, AgentPresence, CodeLocation, DocAnchor, PatchAnchor, ReviewDecision, Participant, Review, ReviewScope, ReviewThread, Severity, ThreadIntent, ThreadOptions, ThreadStatus } from '../common/review-model';
import { ReviewService } from '../common/review-protocol';
import { ReviewClientImpl } from './review-client';
import { AnchorState } from './review-locations';

const ACTIVE_REVIEW_KEY = 'co-review.activeReview';

/** A comment being written. Several can be open at once, in any files; they survive reloads. */
export interface ReviewDraft {
    id: string;
    reviewId: string;
    location: CodeLocation;
    /** What submitting with Ctrl/Cmd+Enter does. */
    defaultIntent: ThreadIntent;
    body: string;
    severity?: Severity;
    /** A suggested edit (document text or patch lines): replace `before` with `after`. */
    proposal?: { before: string; after: string; path?: string; startLine?: number };
    createdAt: number;
}

const DRAFTS_KEY = 'co-review.drafts';

/** Frontend view of the review state of the current workspace. */
@injectable()
export class ReviewManager {

    @inject(ReviewService) protected readonly service: ReviewService;
    @inject(ReviewClientImpl) protected readonly client: ReviewClientImpl;
    @inject(WorkspaceService) protected readonly workspaceService: WorkspaceService;
    @inject(StorageService) protected readonly storage: StorageService;

    protected readonly onDidChangeEmitter = new Emitter<void>();
    readonly onDidChange = this.onDidChangeEmitter.event;
    protected readonly onDidRequestRevealEmitter = new Emitter<string>();
    /** Fired with a thread id when a thread should be shown in the review panel. */
    readonly onDidRequestReveal = this.onDidRequestRevealEmitter.event;

    protected _reviews: Review[] = [];
    protected _activeReviewId: string | undefined;
    protected _root: string | undefined;
    protected _user: Participant = { id: 'human:unknown', kind: 'human', name: 'You' };
    protected _drafts: ReviewDraft[] = [];
    protected saveDraftsTimer: number | undefined;
    /** Thread ids whose inline conversation is collapsed in the editor. */
    protected readonly collapsed = new Set<string>();

    protected readonly onDidChangeDraftsEmitter = new Emitter<void>();
    /** Fired when drafts are added or removed (not on typing). */
    readonly onDidChangeDrafts = this.onDidChangeDraftsEmitter.event;
    protected readonly onDidChangeDraftBodyEmitter = new Emitter<ReviewDraft>();
    readonly onDidChangeDraftBody = this.onDidChangeDraftBodyEmitter.event;
    protected readonly onDidRequestFocusDraftEmitter = new Emitter<string>();
    /** Fired with a draft id when its inline editor should take focus. */
    readonly onDidRequestFocusDraft = this.onDidRequestFocusDraftEmitter.event;
    protected readonly onDidChangeCollapsedEmitter = new Emitter<void>();
    readonly onDidChangeCollapsed = this.onDidChangeCollapsedEmitter.event;
    protected readonly anchorStates = new Map<string, AnchorState>();
    protected readonly presence = new Map<string, AgentPresence>();

    protected _ready: Promise<void>;
    get ready(): Promise<void> { return this._ready; }

    @postConstruct()
    protected init(): void {
        this._ready = this.load();
        this.workspaceService.onWorkspaceChanged(() => this._ready = this.load());
        this.client.onDidChangeReview(review => {
            if (review.workspaceRoot !== this._root) {
                return;
            }
            const index = this._reviews.findIndex(r => r.id === review.id);
            const previous = index >= 0 ? this._reviews[index] : undefined;
            if (index >= 0) {
                this._reviews[index] = review;
            } else {
                this._reviews.unshift(review);
            }
            // An agent from a harness just joined this review: show it.
            if (review.agent?.transport === 'mcp' && previous?.agent?.id !== review.agent.id) {
                this._activeReviewId = review.id;
                this.storage.setData(ACTIVE_REVIEW_KEY, review.id);
            }
            this.onDidChangeEmitter.fire();
        });
        this.client.onDidChangeAgentPresence(p => {
            this.presence.set(p.reviewId, p);
            this.onDidChangeEmitter.fire();
        });
        this.client.onDidDeleteReview(({ reviewId }) => {
            this._reviews = this._reviews.filter(r => r.id !== reviewId);
            if (this._activeReviewId === reviewId) {
                this.setActiveReview(this._reviews[0]?.id);
            }
            this.onDidChangeEmitter.fire();
        });
    }

    protected async load(): Promise<void> {
        const roots = await this.workspaceService.roots;
        this._root = roots[0]?.resource.toString();
        if (!this._root) {
            this._reviews = [];
            this._activeReviewId = undefined;
            this.onDidChangeEmitter.fire();
            return;
        }
        const [user, reviews, active, drafts] = await Promise.all([
            this.service.getCurrentUser(this._root),
            this.service.listReviews(this._root),
            this.storage.getData<string>(ACTIVE_REVIEW_KEY),
            this.storage.getData<ReviewDraft[]>(DRAFTS_KEY, [])
        ]);
        this._user = user;
        this._reviews = reviews;
        // `?review=<id>` (used when an agent opens a review for the reviewer) wins over the last active one.
        const requested = new URLSearchParams(window.location.search).get('review');
        this._activeReviewId = [requested, active].find(id => reviews.some(r => r.id === id)) ?? reviews[0]?.id;
        await Promise.all(reviews.map(async r => {
            const p = await this.service.getAgentPresence(r.id);
            if (p) {
                this.presence.set(r.id, p);
            }
        }));
        this._drafts = (drafts ?? []).filter(d => reviews.some(r => r.id === d.reviewId));
        this.onDidChangeEmitter.fire();
        this.onDidChangeDraftsEmitter.fire();
    }

    get root(): string | undefined { return this._root; }
    get user(): Participant { return this._user; }
    get reviews(): readonly Review[] { return this._reviews; }
    /** Drafts of the active review. */
    get drafts(): readonly ReviewDraft[] {
        return this._drafts.filter(d => d.reviewId === this._activeReviewId);
    }

    draftsForUri(uri: string): ReviewDraft[] {
        return this.drafts.filter(d => d.location.uri === uri);
    }

    get activeReview(): Review | undefined {
        return this._reviews.find(r => r.id === this._activeReviewId);
    }

    /** The repository this window reviews. */
    get workspaceRoot(): string | undefined {
        return this._root;
    }

    /** Whether the active review has an agent to ask; without one, everything is a plain comment. */
    get hasAgent(): boolean {
        return !!this.activeReview?.agent;
    }

    setActiveReview(reviewId: string | undefined): void {
        this._activeReviewId = reviewId;
        this.storage.setData(ACTIVE_REVIEW_KEY, reviewId);
        this.onDidChangeEmitter.fire();
        this.onDidChangeDraftsEmitter.fire();
    }

    async createReview(title: string, scope: ReviewScope): Promise<Review> {
        await this.ready;
        if (!this._root) {
            throw new Error('Open a repository before creating a review.');
        }
        const review = await this.service.createReview({ workspaceRoot: this._root, title, scope, author: this._user });
        if (!this._reviews.some(r => r.id === review.id)) {
            this._reviews.unshift(review);
        }
        this.setActiveReview(review.id);
        return review;
    }

    async deleteReview(reviewId: string): Promise<void> {
        await this.service.deleteReview(reviewId);
    }

    async renameReview(reviewId: string, title: string): Promise<void> {
        await this.service.renameReview(reviewId, title);
    }

    /** Opens a draft at the location, or focuses the existing draft there. */
    addDraft(location: CodeLocation, defaultIntent: ThreadIntent = 'comment', proposal?: ReviewDraft['proposal']): ReviewDraft | undefined {
        const review = this.activeReview;
        if (!review) {
            return undefined;
        }
        if (!this.hasAgent) {
            defaultIntent = 'comment';
        }
        const same = (a: CodeLocation) => a.kind === location.kind && a.uri === location.uri
            && JSON.stringify(a.range) === JSON.stringify(location.range);
        let draft = this.drafts.find(d => same(d.location));
        if (draft) {
            draft.defaultIntent = defaultIntent;
            draft.proposal = proposal ?? draft.proposal;
        } else {
            draft = { id: `draft-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, reviewId: review.id, location, defaultIntent, body: '', proposal, createdAt: Date.now() };
            this._drafts.push(draft);
        }
        this.saveDrafts();
        this.onDidChangeDraftsEmitter.fire();
        this.onDidRequestFocusDraftEmitter.fire(draft.id);
        return draft;
    }

    updateDraft(draftId: string, change: Partial<Pick<ReviewDraft, 'body' | 'severity' | 'defaultIntent' | 'location' | 'proposal'>>): void {
        const draft = this._drafts.find(d => d.id === draftId);
        if (!draft) {
            return;
        }
        Object.assign(draft, change);
        this.saveDrafts();
        this.onDidChangeDraftBodyEmitter.fire(draft);
    }

    discardDraft(draftId: string): void {
        this._drafts = this._drafts.filter(d => d.id !== draftId);
        this.saveDrafts();
        this.onDidChangeDraftsEmitter.fire();
    }

    focusDraft(draftId: string): void {
        this.onDidRequestFocusDraftEmitter.fire(draftId);
    }

    async submitDraft(draftId: string, intent?: ThreadIntent): Promise<ReviewThread | undefined> {
        const draft = this._drafts.find(d => d.id === draftId);
        const suggests = !!draft?.proposal && draft.proposal.after !== draft.proposal.before;
        if (!draft || (!draft.body.trim() && !suggests)) {
            return undefined;
        }
        const options: ThreadOptions = {
            intent: this.hasAgent ? intent ?? draft.defaultIntent : 'comment', severity: draft.severity,
            proposal: suggests ? { ...draft.proposal!, status: 'pending' } : undefined
        };
        const body = draft.body.trim() || 'Suggested edit.';
        const thread = await this.service.createThread(draft.reviewId, draft.location, body, this._user, options);
        this.discardDraft(draftId);
        this.revealThread(thread.id);
        return thread;
    }

    async submitAllDrafts(): Promise<void> {
        for (const draft of this.drafts.filter(d => d.body.trim())) {
            await this.submitDraft(draft.id);
        }
    }

    protected saveDrafts(): void {
        window.clearTimeout(this.saveDraftsTimer);
        this.saveDraftsTimer = window.setTimeout(() => this.storage.setData(DRAFTS_KEY, this._drafts), 300);
    }

    isCollapsed(threadId: string): boolean {
        return this.collapsed.has(threadId);
    }

    setCollapsedMany(threadIds: string[], collapsed: boolean): void {
        for (const id of threadIds) {
            if (collapsed) {
                this.collapsed.add(id);
            } else {
                this.collapsed.delete(id);
            }
        }
        this.onDidChangeCollapsedEmitter.fire();
    }

    setCollapsed(threadId: string, collapsed: boolean): void {
        if (collapsed !== this.collapsed.has(threadId)) {
            if (collapsed) {
                this.collapsed.add(threadId);
            } else {
                this.collapsed.delete(threadId);
            }
            this.onDidChangeCollapsedEmitter.fire();
        }
    }

    async reply(thread: ReviewThread, body: string): Promise<void> {
        const review = this.activeReview;
        if (review && body.trim()) {
            await this.service.addMessage(review.id, thread.id, body.trim(), this._user);
        }
    }

    async setStatus(thread: ReviewThread, status: ThreadStatus): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.setThreadStatus(review.id, thread.id, status, this._user);
        }
    }

    async relocate(thread: ReviewThread, location: CodeLocation): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.relocateThread(review.id, thread.id, location, this._user);
        }
    }

    async decideProposal(thread: ReviewThread, accept: boolean): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.decideProposal(review.id, thread.id, accept);
        }
    }

    async submitReview(decision: ReviewDecision, summary: string): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.submitReview(review.id, decision, summary, this._user);
        }
    }

    /** Pulls in pre-seeded findings of the review directory (idempotent). */
    async syncFindings(): Promise<void> {
        const review = this.activeReview;
        if (review?.bundle) {
            await this.service.syncFindings(review.id);
        }
    }

    getAgentPresence(reviewId: string): AgentPresence | undefined {
        return this.presence.get(reviewId);
    }

    getAgentPresets(): Promise<AgentConfig[]> {
        return this.service.getAgentPresets();
    }

    async setAgent(agent: AgentConfig | undefined): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.setAgent(review.id, agent);
        }
    }

    async askAgent(thread: ReviewThread): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.askAgent(review.id, thread.id);
        }
    }

    async cancelAgent(thread: ReviewThread): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.cancelAgent(review.id, thread.id);
        }
    }

    answerPermission(requestId: string, optionId: string): Promise<void> {
        return this.service.answerPermission(requestId, optionId);
    }

    revealThread(threadId: string): void {
        this.onDidRequestRevealEmitter.fire(threadId);
    }

    threadsForUri(uri: string): ReviewThread[] {
        return this.activeReview?.threads.filter(t => t.location.uri === uri) ?? [];
    }

    getAnchorState(threadId: string): AnchorState | undefined {
        return this.anchorStates.get(threadId);
    }

    setAnchorState(threadId: string, state: AnchorState): void {
        if (this.anchorStates.get(threadId) !== state) {
            this.anchorStates.set(threadId, state);
            this.onDidChangeEmitter.fire();
        }
    }

    relativePath(uri: string): string {
        if (!this._root) {
            return uri;
        }
        const relative = new URI(this._root).relative(new URI(uri));
        return relative ? (relative.toString() || '.') : new URI(uri).path.fsPath();
    }

    locationLabel(location: CodeLocation): string {
        if (location.kind === 'repository' || !location.uri) {
            return 'Repository';
        }
        const path = this.relativePath(location.uri);
        if (location.kind === 'patch' && location.patchAnchor) {
            return location.patchAnchor.type === 'patch' ? path : `${path} · ${PatchAnchor.describe(location.patchAnchor)}`;
        }
        if (location.kind === 'document' && location.docAnchor) {
            return location.docAnchor.type === 'document' ? path : `${path} · ${DocAnchor.describe(location.docAnchor)}`;
        }
        if (!location.range) {
            return location.kind === 'directory' ? `${path}/` : path;
        }
        const start = location.range.start.line + 1;
        const end = location.range.end.line + 1;
        const lines = start === end ? `${start}` : `${start}-${end}`;
        return `${path}:${lines}`;
    }
}
