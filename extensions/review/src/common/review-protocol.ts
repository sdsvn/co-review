import { RpcServer } from '@theia/core/lib/common/messaging';
import { AgentConfig, AgentSetting, AgentPresence, CodeLocation, ReviewDecision, Participant, Review, ReviewCoverage, ReviewScope, ReviewThread, ThreadOptions, ThreadStatus } from './review-model';

export const REVIEW_SERVICE_PATH = '/services/co-review';
export const ReviewService = Symbol('ReviewService');
export const ReviewClient = Symbol('ReviewClient');

export interface CreateReviewParams {
    workspaceRoot: string;
    title: string;
    scope: ReviewScope;
    author: Participant;
}

export interface GitInfo {
    isRepository: boolean;
    currentBranch?: string;
    branches: string[];
}

/**
 * Backend-owned review state. All mutations go through the backend so that other
 * participants (e.g. an agent runtime living in the backend) observe the same state,
 * and every connected frontend is notified through {@link ReviewClient}.
 */
/** How an agent harness launches Co-Review's MCP server. */
export interface McpLaunch {
    command: string;
    args: string[];
    env?: Record<string, string>;
}

export interface SkillTarget {
    id: string;
    label: string;
    dir: string;
}

/** Co-Review's MCP server in one harness's config format. `json` / `toml` / `cli` can be applied; `manual` is copy-only. */
export interface HarnessSetup {
    id: string;
    label: string;
    kind: 'cli' | 'json' | 'toml' | 'manual';
    snippet: string;
    file?: string;
    /** For `json`: the top-level key that holds the servers. */
    key?: string[];
    /** Shown under the label. */
    detail?: string;
    /** For `cli`: the commands to run, in order. */
    argv?: string[][];
}

export interface AgentSetupInfo {
    launch: McpLaunch;
    skills: SkillTarget[];
    skillsAvailable: boolean;
    harnesses: HarnessSetup[];
    /** Whether the `co-review` command is installed (the Claude Code plugin, Pi and Oh My Pi need it). */
    cliInstalled: boolean;
}

export interface ReviewService extends RpcServer<ReviewClient> {
    getCurrentUser(workspaceRoot: string): Promise<Participant>;
    getGitInfo(workspaceRoot: string): Promise<GitInfo>;
    resolveCommit(workspaceRoot: string, ref: string): Promise<string | undefined>;

    listReviews(workspaceRoot: string): Promise<Review[]>;
    getReview(reviewId: string): Promise<Review | undefined>;
    createReview(params: CreateReviewParams): Promise<Review>;
    renameReview(reviewId: string, title: string): Promise<Review>;
    deleteReview(reviewId: string): Promise<void>;
    /** Archive (or unarchive) a review: hidden from the active list, kept on disk. */
    archiveReview(reviewId: string, archived: boolean): Promise<Review>;

    createThread(reviewId: string, location: CodeLocation, body: string, author: Participant, options?: ThreadOptions): Promise<ReviewThread>;
    addMessage(reviewId: string, threadId: string, body: string, author: Participant): Promise<ReviewThread>;
    setThreadStatus(reviewId: string, threadId: string, status: ThreadStatus, actor: Participant): Promise<ReviewThread>;
    relocateThread(reviewId: string, threadId: string, location: CodeLocation, actor: Participant): Promise<ReviewThread>;

    /** ACP agents available on this machine. */
    getAgentPresets(): Promise<AgentConfig[]>;
    /** Sets (or removes) the ACP agent participating in the review. */
    setAgent(reviewId: string, agent: AgentConfig | undefined): Promise<Review>;
    /** What the review's ACP agent lets you choose for its sessions (models, reasoning effort, modes); starts it if needed. */
    getAgentSettings(reviewId: string): Promise<AgentSetting[]>;
    /** Marks repository-relative files as viewed (or not). */
    setViewed(reviewId: string, paths: string[], viewed: boolean): Promise<Review>;
    /** How much of the repository's source the reviewer has viewed, overall and per area. */
    getCoverage(reviewId: string): Promise<ReviewCoverage | undefined>;
    /** Opens a local HTML page in the system's default browser (pages are not rendered inside Co-Review). */
    openInBrowser(uri: string): Promise<void>;
    /** Writes the repository overview page (where to start, areas, how they connect) and returns its file URI. */
    writeOverview(reviewId: string): Promise<string | undefined>;
    /** Sends the thread to the review's agent (marks it as a question). */
    askAgent(reviewId: string, threadId: string): Promise<void>;
    cancelAgent(reviewId: string, threadId: string): Promise<void>;
    answerPermission(requestId: string, optionId: string): Promise<void>;
    getAgentPresence(reviewId: string): Promise<AgentPresence | undefined>;
    /** Submit the review round (decision + message to the agent). */
    submitReview(reviewId: string, decision: ReviewDecision, summary: string, actor: Participant): Promise<Review>;
    /** Accept or reject a suggested edit. Accepted document edits are written into the document. */
    decideProposal(reviewId: string, threadId: string, accept: boolean): Promise<void>;
    /** Ingests pre-seeded findings (`<patch>.comments.json[l]` sidecars) of a review directory. */
    syncFindings(reviewId: string): Promise<number>;

    /** Skills and MCP setup for agent harnesses. */
    getAgentSetup(workspaceRoot?: string): Promise<AgentSetupInfo>;
    /** Copies the bundled skills into a skills directory; returns the installed paths. */
    installSkills(targetId: string, workspaceRoot?: string): Promise<string[]>;
    /** Adds Co-Review's MCP server to a harness's config; returns what was done. */
    applyHarnessSetup(harnessId: string): Promise<string>;
    /** Installs the `co-review` and `co-review-server` commands; returns the installer's output. */
    installCli(): Promise<string>;
}

export interface ReviewClient {
    onAgentPresence(presence: AgentPresence): void;
    onReviewChanged(review: Review): void;
    onReviewDeleted(reviewId: string, workspaceRoot: string): void;
}
