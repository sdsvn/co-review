import { Emitter } from '@theia/core/lib/common/event';
import URI from '@theia/core/lib/common/uri';
import { StorageService } from '@theia/core/lib/browser/storage-service';
import { ConnectionStatus, ConnectionStatusService } from '@theia/core/lib/browser/connection-status-service';
import { MessageService } from '@theia/core/lib/common/message-service';
import { FileService } from '@theia/filesystem/lib/browser/file-service';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import { WorkspaceService } from '@theia/workspace/lib/browser/workspace-service';
import { PreferenceService } from '@theia/core/lib/common/preferences';
import { AgentConfig, AgentSetting, AgentPresence, CodeLocation, DocAnchor, PatchAnchor, ReviewCoverage, ReviewDecision, Participant, Review, ReviewScope, ReviewThread, Severity, ThreadIntent, ThreadOptions, ThreadStatus, isOverviewPage } from '../common/review-model';
import { GitHubEdits, GitHubPost, GitHubPreview, GitHubTarget, ReviewService } from '../common/review-protocol';
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
/** Per review: thread id → when the reviewer last read it (opened or selected it), for unread agent replies. */
const READ_KEY = 'co-review.read';
/** The preference that turns OS notifications for agent replies on or off. */
export const NOTIFICATIONS_PREFERENCE = 'co-review.notifications';

/** Frontend view of the review state of the current workspace. */
@injectable()
export class ReviewManager {

    @inject(ReviewService) protected readonly service: ReviewService;
    @inject(ReviewClientImpl) protected readonly client: ReviewClientImpl;
    @inject(WorkspaceService) protected readonly workspaceService: WorkspaceService;
    @inject(StorageService) protected readonly storage: StorageService;
    @inject(MessageService) protected readonly messages: MessageService;
    @inject(ConnectionStatusService) protected readonly connection: ConnectionStatusService;
    @inject(FileService) protected readonly fileService: FileService;
    @inject(PreferenceService) protected readonly preferences: PreferenceService;
    protected refreshTimer: number | undefined;
    protected readonly onDidRequestSubmitEmitter = new Emitter<void>();
    /** The reviewer asked to submit (the overview's button, a shortcut): the panel opens its submit form. */
    readonly onDidRequestSubmit = this.onDidRequestSubmitEmitter.event;
    /** Thread id → when the reviewer last read it; replies after that are unread. */
    protected read: Record<string, number> = {};
    /** Before any read state was stored: what came earlier counts as read (no wall of unread dots on first use). */
    protected readSince = Date.now();
    protected saveReadTimer: number | undefined;
    /** When this window last notified about each review, to send at most one notification a minute per review. */
    protected readonly notifiedAt = new Map<string, number>();

    protected readonly onDidChangeEmitter = new Emitter<void>();
    readonly onDidChange = this.onDidChangeEmitter.event;
    protected readonly onDidRequestRevealEmitter = new Emitter<string>();
    /** Fired with a thread id when a thread should be shown in the review panel. */
    readonly onDidRequestReveal = this.onDidRequestRevealEmitter.event;

    protected _reviews: Review[] = [];
    protected _activeReviewId: string | undefined;
    protected _root: string | undefined;
    /** The review this window was opened for (`?review=<id>`, when an agent showed it): each such review has its own window. */
    protected readonly openedFor = new URLSearchParams(window.location.search).get('review') ?? undefined;

    /** The review this window was opened for by an agent (its own window), if it was. */
    get windowReviewId(): string | undefined {
        return this.openedFor;
    }
    protected _user: Participant = { id: 'human:unknown', kind: 'human', name: 'You' };
    protected _drafts: ReviewDraft[] = [];
    protected saveDraftsTimer: number | undefined;
    /** Thread ids whose inline conversation is collapsed in the editor. */
    protected readonly collapsed = new Set<string>();
    /** Thread ids the reviewer opened (expanded) this session: a proposed finding is only shown inline once opened. */
    protected readonly opened = new Set<string>();

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
            // The window's own reviews, and change reviews whose code folder it shows.
            if (review.workspaceRoot !== this._root && !(review.bundle?.code && URI.fromFilePath(review.bundle.code).toString() === this._root)) {
                return;
            }
            const index = this._reviews.findIndex(r => r.id === review.id);
            const previous = index >= 0 ? this._reviews[index] : undefined;
            if (index >= 0) {
                this._reviews[index] = review;
            } else {
                this._reviews.unshift(review);
            }
            this.notify(previous, review);
            this.onDidChangeEmitter.fire();
        });
        this.client.onDidChangeAgentPresence(p => {
            this.presence.set(p.reviewId, p);
            this.onDidChangeEmitter.fire();
        });
        // The agent that showed this window is done with its review (desktop app started by an agent).
        this.client.onDidRequestCloseWindow(({ workspaceRoot, message, reviewId }) =>
            workspaceRoot === this._root && reviewId === this.openedFor && this.closeSoon(message));
        // A commented file changed (an agent's edit, a save): the backend carries its comments through the change.
        this.fileService.onDidFilesChange(e => {
            const review = this.activeReview;
            if (review?.threads.some(t => t.location.tracked && t.location.uri && e.contains(new URI(t.location.uri)))) {
                window.clearTimeout(this.refreshTimer);
                this.refreshTimer = window.setTimeout(() => this.service.refreshAnchors(review.id).catch(() => undefined), 300);
            }
        });
        // Tell the backend again after a reconnect: its agents are told when this window closes.
        this.connection.onStatusChange(status => status === ConnectionStatus.ONLINE && this._root && this.service.showWorkspace(this._root, this.openedFor));
        this.client.onDidDeleteReview(({ reviewId }) => {
            this._reviews = this._reviews.filter(r => r.id !== reviewId);
            this.presence.delete(reviewId);
            if (this._activeReviewId === reviewId) {
                this.setActiveReview(this._reviews[0]?.id);
            }
            this.onDidChangeEmitter.fire();
        });
    }

    /** Says why the window closes (`message`), then closes it after a countdown unless the reviewer keeps it open. */
    protected async closeSoon(message: string): Promise<void> {
        const seconds = 10;
        const countdown = new Promise<'close'>(resolve => setTimeout(() => resolve('close'), seconds * 1000));
        const choice = await Promise.race([
            this.messages.info(`${message} This window closes in ${seconds} seconds.`, { timeout: 0 }, 'Keep open', 'Close now'),
            countdown
        ]);
        // Only the countdown or Close now closes it: dismissing the notice keeps the window, like Keep open.
        if (choice === 'close' || choice === 'Close now') {
            window.close();
        } else if (this._root) {
            this.service.keepWindowOpen(this._root, this.openedFor);
        }
    }

    protected async load(): Promise<void> {
        const roots = await this.workspaceService.roots;
        this._root = roots[0]?.resource.toString();
        if (this._root) {
            this.service.showWorkspace(this._root, this.openedFor);
        }
        if (!this._root) {
            this._reviews = [];
            this._activeReviewId = undefined;
            this.onDidChangeEmitter.fire();
            return;
        }
        const [user, reviews, active, drafts, read] = await Promise.all([
            this.service.getCurrentUser(this._root),
            this.service.listReviews(this._root),
            this.storage.getData<string>(ACTIVE_REVIEW_KEY),
            this.storage.getData<ReviewDraft[]>(DRAFTS_KEY, []),
            this.storage.getData<{ since: number; read: Record<string, number> }>(READ_KEY)
        ]);
        if (read) {
            this.readSince = read.since;
            this.read = read.read ?? {};
        } else {
            this.storage.setData(READ_KEY, { since: this.readSince, read: this.read });
        }
        this._user = user;
        this._reviews = reviews;
        // `?review=<id>` (used when an agent opens a review for the reviewer) wins over the last active one.
        this._activeReviewId = [this.openedFor, active].find(id => reviews.some(r => r.id === id)) ?? reviews.find(r => !r.archivedAt)?.id;
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

    /**
     * Tells the reviewer, while this window is in the background, that the agent replied or added findings: an OS
     * notification (clicking it brings the window and the thread), at most one a minute per review, unless the
     * `co-review.notifications` preference is off. Only for the review this window shows.
     */
    protected notify(previous: Review | undefined, review: Review): void {
        if (!previous || document.hasFocus() || review.id !== this._activeReviewId || typeof Notification === 'undefined'
            || this.preferences.get<boolean>(NOTIFICATIONS_PREFERENCE, true) === false) {
            return;
        }
        const done = (t: ReviewThread) => t.messages.filter(m => m.author.kind === 'agent' && m.status !== 'streaming').length;
        const replied = review.threads.find(t => {
            const before = previous.threads.find(p => p.id === t.id);
            return before && done(t) > done(before);
        });
        const added = review.threads.filter(t => !previous.threads.some(p => p.id === t.id) && t.messages[0]?.author.kind === 'agent');
        if (!replied && !added.length) {
            return;
        }
        const now = Date.now();
        if (now - (this.notifiedAt.get(review.id) ?? 0) < 60_000) {
            return;
        }
        this.notifiedAt.set(review.id, now);
        const agent = review.agent?.name ?? 'The agent';
        const thread = replied ?? added[0];
        const body = replied
            ? `${agent} replied in #${replied.number}: ${replied.messages[replied.messages.length - 1].body.replace(/\s+/g, ' ').slice(0, 120)}`
            : `${agent} added ${added.length} finding${added.length === 1 ? '' : 's'}.`;
        try {
            const notification = new Notification(review.title, { body, tag: `co-review:${review.id}` });
            notification.onclick = () => {
                window.focus();
                this.revealThread(thread.id);
            };
        } catch {
            /* notifications not permitted */
        }
    }

    /** Whether the agent replied in a thread since the reviewer last read it. */
    isUnread(thread: ReviewThread): boolean {
        const last = thread.messages[thread.messages.length - 1];
        if (!last || last.author.kind !== 'agent' || last.status === 'streaming' || thread.messages.length < 2) {
            return false;
        }
        return last.createdAt > (this.read[thread.id] ?? this.readSince);
    }

    /** The reviewer read the thread (opened, selected or expanded it). */
    markRead(threadId: string): void {
        const thread = this.activeReview?.threads.find(t => t.id === threadId);
        if (!thread || !this.isUnread(thread)) {
            return;
        }
        this.read[threadId] = Date.now();
        window.clearTimeout(this.saveReadTimer);
        this.saveReadTimer = window.setTimeout(() => this.storage.setData(READ_KEY, { since: this.readSince, read: this.read }), 500);
        this.onDidChangeEmitter.fire();
    }

    /** Opens the submit form in the Review panel. */
    requestSubmit(): void {
        // An explanation has nothing to submit: its window closes when the reviewer is done.
        if (this.activeReview?.purpose !== 'explain') {
            this.onDidRequestSubmitEmitter.fire();
        }
    }

    /**
     * The reviewer is done with an explanation: the window closes, and its agent is told (a closed review is done). A
     * browser tab a script did not open cannot close itself, so the reviewer is told to close it.
     */
    async finish(): Promise<void> {
        const review = this.activeReview;
        if (review) {
            await this.service.finishReview(review.id);
        }
        window.close();
        // Still here: a browser tab Co-Review did not open, which only the reviewer can close.
        setTimeout(() => this.messages.info(`Done: ${review?.agent?.name ?? 'the agent'} knows you are finished. You can close this tab.`), 300);
    }

    get root(): string | undefined { return this._root; }
    get user(): Participant { return this._user; }
    /** Active (non-archived) reviews, for the switcher. */
    get reviews(): readonly Review[] { return this._reviews.filter(r => !r.archivedAt); }
    /** Archived reviews, most recently archived first. */
    get archivedReviews(): readonly Review[] {
        return this._reviews.filter(r => r.archivedAt).sort((a, b) => (b.archivedAt ?? 0) - (a.archivedAt ?? 0));
    }
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
        if (reviewId !== this._activeReviewId) {
            // Anchor states belong to the threads on screen; the new review's views compute their own.
            this.anchorStates.clear();
        }
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

    /** Archive (or unarchive) a review. Archiving the active one switches to the next; unarchiving activates it. */
    async archiveReview(reviewId: string, archived: boolean): Promise<void> {
        const updated = await this.service.archiveReview(reviewId, archived);
        const index = this._reviews.findIndex(r => r.id === reviewId);
        if (index >= 0) {
            this._reviews[index] = updated;
        }
        if (!archived) {
            this.setActiveReview(reviewId);
        } else if (this._activeReviewId === reviewId) {
            this.setActiveReview(this._reviews.find(r => !r.archivedAt)?.id);
        } else {
            this.onDidChangeEmitter.fire();
        }
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

    /** Whether the reviewer opened the thread this session (and hasn't folded it since). */
    isOpened(threadId: string): boolean {
        return this.opened.has(threadId);
    }

    setCollapsedMany(threadIds: string[], collapsed: boolean): void {
        for (const id of threadIds) {
            if (collapsed) {
                this.collapsed.add(id);
                this.opened.delete(id);
            } else {
                this.collapsed.delete(id);
                this.opened.add(id);
            }
        }
        this.onDidChangeCollapsedEmitter.fire();
    }

    setCollapsed(threadId: string, collapsed: boolean): void {
        if (collapsed !== this.collapsed.has(threadId) || collapsed === this.opened.has(threadId)) {
            if (collapsed) {
                this.collapsed.add(threadId);
                this.opened.delete(threadId);
            } else {
                this.collapsed.delete(threadId);
                this.opened.add(threadId);
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

    /** The GitHub pull request of the active review (a review directory with a PR.md naming it), if any. */
    getGitHubTarget(): Promise<GitHubTarget | undefined> {
        const review = this.activeReview;
        return review?.bundle ? this.service.getGitHubTarget(review.id).catch(() => undefined) : Promise.resolve(undefined);
    }

    /** What posting the active review to GitHub would send (the latest verdict unless given). */
    previewGitHubPost(decision?: ReviewDecision, summary?: string): Promise<GitHubPreview> {
        const review = this.activeReview;
        return review ? this.service.previewGitHubPost(review.id, decision, summary) : Promise.reject(new Error('No review.'));
    }

    /** Posts the active review to its GitHub pull request (the latest verdict unless given), with the reviewer's edits to its preview. */
    postToGitHub(decision?: ReviewDecision, summary?: string, edits?: GitHubEdits): Promise<GitHubPost> {
        const review = this.activeReview;
        return review ? this.service.postToGitHub(review.id, decision, summary, edits) : Promise.reject(new Error('No review.'));
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

    /** Whether the reviewer is done with the file (a URI): viewed it, or marked it "don't care". */
    isDone(uri: string): boolean {
        return this.pathState(this.relativePath(uri)) !== undefined;
    }

    /** A repository-relative file's mark in the active review: viewed, "don't care" (`skipped`), or none. */
    pathState(path: string): 'viewed' | 'skipped' | undefined {
        const review = this.activeReview;
        return review?.viewed?.[path] ? 'viewed' : review?.skipped?.[path] ? 'skipped' : undefined;
    }

    async setViewed(uris: string[], viewed: boolean | 'skipped'): Promise<void> {
        await this.setPathState(uris.map(u => this.relativePath(u)), viewed);
    }

    /** Marks repository-relative files viewed, "don't care" (`'skipped'`), or neither (`false`). */
    async setPathState(paths: string[], state: boolean | 'skipped'): Promise<void> {
        const review = this.activeReview;
        if (review && paths.length) {
            await this.service.setViewed(review.id, paths, state);
        }
    }

    /** Writes the active review's overview page (kept up to date by the backend); its file URI. */
    writeOverview(): Promise<string | undefined> {
        const review = this.activeReview;
        return review ? this.service.writeOverview(review.id) : Promise.resolve(undefined);
    }

    getCoverage(): Promise<ReviewCoverage | undefined> {
        const review = this.activeReview;
        return review ? this.service.getCoverage(review.id) : Promise.resolve(undefined);
    }

    getAgentSettings(): Promise<AgentSetting[]> {
        const review = this.activeReview;
        return review ? this.service.getAgentSettings(review.id) : Promise.resolve([]);
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
        return [...this.activeReview?.threads.filter(t => t.location.uri === uri) ?? [], ...this.diffThreadsIn(uri)];
    }

    /** Copies of diff threads placed in the head file (see diffThreadsIn): shown there, never saved from there. */
    protected readonly placed = new WeakSet<ReviewThread>();

    isPlacedFromDiff(thread: ReviewThread): boolean {
        return this.placed.has(thread);
    }

    /**
     * A change review's threads on new-side lines of its diff, placed at the same lines of the file in its code
     * folder, so opening the file from the diff shows the conversation there too.
     */
    protected diffThreadsIn(uri: string): ReviewThread[] {
        const review = this.activeReview;
        const code = review?.bundle?.code;
        if (!review || !code) {
            return [];
        }
        const folder = URI.fromFilePath(code);
        return review.threads.flatMap(t => {
            const a = t.location.patchAnchor;
            const start = a?.type === 'code-line' ? a.line : a?.type === 'code-range' ? a.startLine : undefined;
            const end = a?.type === 'code-range' ? a.endLine : start;
            if (!a?.path || (a.side ?? 'new') !== 'new' || !start || !end || folder.resolve(a.path).toString() !== uri) {
                return [];
            }
            const copy: ReviewThread = {
                ...t, location: {
                    kind: start === end ? 'line' : 'range', uri,
                    range: { start: { line: start - 1, character: 0 }, end: { line: end - 1, character: Number.MAX_SAFE_INTEGER } },
                    anchor: { text: a.source ?? t.location.anchor?.text ?? '' }
                }
            };
            this.placed.add(copy);
            return [copy];
        });
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
        if (isOverviewPage(uri)) {
            return 'Overview page';
        }
        if (!this._root) {
            return uri;
        }
        const relative = new URI(this._root).relative(new URI(uri));
        return relative ? (relative.toString() || '.') : new URI(uri).path.fsPath();
    }

    /**
     * The file a comment on a change review's diff is about (a file, line or range of the patch), when the review has
     * its code: the path is relative to the code folder, which is the window's root.
     */
    patchFile(location: CodeLocation): { path: string; line?: number; endLine?: number; side: 'new' | 'old' } | undefined {
        const a = location.kind === 'patch' ? location.patchAnchor : undefined;
        if (!a?.path || a.type === 'patch' || !this.activeReview?.bundle?.code) {
            return undefined;
        }
        const line = a.type === 'code-line' ? a.line : a.type === 'code-range' ? a.startLine : undefined;
        return { path: a.path, line, endLine: a.type === 'code-range' ? a.endLine : line, side: a.side ?? 'new' };
    }

    locationLabel(location: CodeLocation): string {
        if (location.kind === 'repository' || !location.uri) {
            return 'Repository';
        }
        const file = this.patchFile(location);
        if (file) {
            const lines = !file.line ? '' : file.endLine && file.endLine !== file.line ? `:${file.line}-${file.endLine}` : `:${file.line}`;
            return `${file.path}${lines}${file.side === 'old' ? ' (old)' : ''}`;
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
