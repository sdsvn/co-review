import { DisposableCollection } from '@theia/core/lib/common/disposable';
import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable } from '@theia/core/shared/inversify';
import { execFile } from 'child_process';
import { AgentConfig, AgentSetting, AgentPresence, CodeLocation, ReviewDecision, Participant, Review, ReviewCoverage, ReviewThread, ThreadOptions, ThreadStatus } from '../common/review-model';
import { AcpAgentService } from './acp-agent-service';
import { BundleService } from './bundle-service';
import { currentUser } from './participants';
import { AgentPresenceTracker, HumanDecisions } from './agent-coordination';
import { AgentSetupInfo, CreateReviewParams, GitInfo, ReviewClient, ReviewService } from '../common/review-protocol';
import { AgentSetup } from './agent-setup';
import { RepoIndex } from './repo-index';
import { ReviewStore } from './review-store';

function git(cwd: string, args: string[]): Promise<string | undefined> {
    return new Promise(resolve => execFile('git', args, { cwd }, (err, stdout) => resolve(err ? undefined : stdout.trim())));
}

/** One instance per frontend connection; forwards store changes to its client. */
@injectable()
export class ReviewServiceImpl implements ReviewService {

    @inject(ReviewStore)
    protected readonly store: ReviewStore;

    @inject(AcpAgentService)
    protected readonly agents: AcpAgentService;

    @inject(HumanDecisions)
    protected readonly decisions: HumanDecisions;

    @inject(AgentSetup)
    protected readonly setup: AgentSetup;

    @inject(BundleService)
    protected readonly bundles: BundleService;

    @inject(AgentPresenceTracker)
    protected readonly presence: AgentPresenceTracker;

    @inject(RepoIndex)
    protected readonly index: RepoIndex;

    protected client: ReviewClient | undefined;
    protected readonly toDispose = new DisposableCollection();

    setClient(client: ReviewClient | undefined): void {
        this.client = client;
        this.toDispose.dispose();
        if (client) {
            this.toDispose.push(this.presence.onDidChange(p => client.onAgentPresence(p)));
            this.toDispose.push(this.store.onDidChange(change => {
                if (change.kind === 'changed') {
                    client.onReviewChanged(change.review);
                } else {
                    client.onReviewDeleted(change.reviewId, change.workspaceRoot);
                }
            }));
        }
    }

    dispose(): void {
        this.toDispose.dispose();
    }

    getCurrentUser(workspaceRoot: string): Promise<Participant> {
        return currentUser(workspaceRoot);
    }

    async getGitInfo(workspaceRoot: string): Promise<GitInfo> {
        const cwd = FileUri.fsPath(workspaceRoot);
        if (await git(cwd, ['rev-parse', '--is-inside-work-tree']) !== 'true') {
            return { isRepository: false, branches: [] };
        }
        const currentBranch = await git(cwd, ['rev-parse', '--abbrev-ref', 'HEAD']);
        const branches = (await git(cwd, ['for-each-ref', '--format=%(refname:short)', 'refs/heads', 'refs/remotes']) ?? '')
            .split('\n').filter(b => b && !b.endsWith('/HEAD'));
        return { isRepository: true, currentBranch, branches };
    }

    resolveCommit(workspaceRoot: string, ref: string): Promise<string | undefined> {
        return git(FileUri.fsPath(workspaceRoot), ['rev-parse', '--verify', '--quiet', `${ref}^{commit}`]);
    }

    listReviews(workspaceRoot: string): Promise<Review[]> {
        return this.store.list(workspaceRoot);
    }

    getReview(reviewId: string): Promise<Review | undefined> {
        return this.store.get(reviewId);
    }

    createReview(params: CreateReviewParams): Promise<Review> {
        return this.store.create(params);
    }

    renameReview(reviewId: string, title: string): Promise<Review> {
        return this.store.rename(reviewId, title);
    }

    deleteReview(reviewId: string): Promise<void> {
        return this.store.delete(reviewId);
    }

    archiveReview(reviewId: string, archived: boolean): Promise<Review> {
        return this.store.archive(reviewId, archived);
    }

    createThread(reviewId: string, location: CodeLocation, body: string, author: Participant, options?: ThreadOptions): Promise<ReviewThread> {
        return this.store.createThread(reviewId, location, body, author, options);
    }

    addMessage(reviewId: string, threadId: string, body: string, author: Participant): Promise<ReviewThread> {
        return this.store.addMessage(reviewId, threadId, body, author);
    }

    setThreadStatus(reviewId: string, threadId: string, status: ThreadStatus, actor: Participant): Promise<ReviewThread> {
        return this.store.setThreadStatus(reviewId, threadId, status, actor);
    }

    relocateThread(reviewId: string, threadId: string, location: CodeLocation, actor: Participant): Promise<ReviewThread> {
        return this.store.relocateThread(reviewId, threadId, location, actor);
    }

    getAgentPresets(): Promise<AgentConfig[]> {
        return this.agents.getPresets();
    }

    setAgent(reviewId: string, agent: AgentConfig | undefined): Promise<Review> {
        return this.store.setAgent(reviewId, agent);
    }

    getAgentSettings(reviewId: string): Promise<AgentSetting[]> {
        return this.agents.getSettings(reviewId);
    }

    setViewed(reviewId: string, paths: string[], viewed: boolean): Promise<Review> {
        return this.store.setViewed(reviewId, paths, viewed);
    }

    async writeOverview(reviewId: string): Promise<string | undefined> {
        const review = await this.store.get(reviewId);
        if (!review || review.bundle) {
            return undefined;
        }
        return FileUri.create(await this.index.writeOverview(FileUri.fsPath(review.workspaceRoot), review.viewed)).toString();
    }

    async openInBrowser(uri: string): Promise<void> {
        const file = FileUri.fsPath(uri);
        if (!uri.startsWith('file:') || !/\.html?$/i.test(file)) {
            throw new Error(`Not a local HTML page: ${uri}`);
        }
        const opener = process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'explorer' : 'xdg-open';
        execFile(opener, [file], () => undefined);
    }

    async getCoverage(reviewId: string): Promise<ReviewCoverage | undefined> {
        const review = await this.store.get(reviewId);
        // Review directories (designs, patches) are not repository reviews.
        return review && !review.bundle ? this.index.coverage(FileUri.fsPath(review.workspaceRoot), review.viewed) : undefined;
    }

    async askAgent(reviewId: string, threadId: string): Promise<void> {
        await this.store.setThreadIntent(reviewId, threadId, 'question');
        this.agents.ask(reviewId, threadId);
    }

    cancelAgent(reviewId: string, threadId: string): Promise<void> {
        return this.agents.cancel(reviewId, threadId);
    }

    async answerPermission(requestId: string, optionId: string): Promise<void> {
        this.decisions.resolve(requestId, optionId);
    }

    async getAgentPresence(reviewId: string): Promise<AgentPresence | undefined> {
        return this.presence.get(reviewId);
    }

    submitReview(reviewId: string, decision: ReviewDecision, summary: string, actor: Participant): Promise<Review> {
        return this.store.submit(reviewId, decision, summary, actor);
    }

    syncFindings(reviewId: string): Promise<number> {
        return this.bundles.syncFindings(reviewId);
    }

    decideProposal(reviewId: string, threadId: string, accept: boolean): Promise<void> {
        return this.bundles.decideProposal(reviewId, threadId, accept);
    }

    async getAgentSetup(workspaceRoot?: string): Promise<AgentSetupInfo> {
        return this.setup.info(workspaceRoot);
    }

    installSkills(targetId: string, workspaceRoot?: string): Promise<string[]> {
        return this.setup.installSkills(targetId, workspaceRoot);
    }

    applyHarnessSetup(harnessId: string): Promise<string> {
        return this.setup.apply(harnessId);
    }

    installCli(): Promise<string> {
        return this.setup.installCli();
    }
}
