/** Zero-based position, same convention as LSP. */
export interface Position {
    line: number;
    character: number;
}

export interface Range {
    start: Position;
    end: Position;
}

export type LocationKind = 'repository' | 'directory' | 'file' | 'symbol' | 'line' | 'range' | 'document' | 'patch';

/**
 * An anchor inside a rendered document (Markdown / pseudocode / Mermaid). The shape is
 * The documented anchor format (llms.txt), stored verbatim so agent payloads round-trip.
 */
export interface DocAnchor {
    type: 'document' | 'text' | 'mermaid-block' | 'mermaid-node' | 'mermaid-edge' | 'tree-node';
    exact?: string;
    prefix?: string;
    suffix?: string;
    startOffsetHint?: number;
    endOffsetHint?: number;
    blockId?: string;
    nodeId?: string;
    edgeId?: string;
    /** Mermaid node source (`id[label]`) or the tree step text. */
    source?: string;
    /**
     * The version of the document the comment was made on ({@link contentVersion} of its file). Once the document
     * changes, the comment is about text that may be gone or moved: it is no longer placed on the document, and shows
     * in the Review panel as on an earlier version. Not set on generated pages (the overview), which change all the time.
     */
    version?: string;
}

/** A document's version: a short hash of its content (FNV-1a, with the length), the same in the backend and the browser. */
export function contentVersion(text: string): string {
    let hash = 0x811c9dc5;
    for (let i = 0; i < text.length; i++) {
        hash ^= text.charCodeAt(i);
        hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    return `${hash.toString(16).padStart(8, '0')}-${text.length.toString(36)}`;
}

/** An anchor in a patch review page: the patch, a file, a line or a range. */
export interface PatchAnchor {
    type: 'patch' | 'code-file' | 'code-line' | 'code-range';
    path?: string;
    line?: number;
    startLine?: number;
    endLine?: number;
    side?: 'new' | 'old';
    /** The diff line(s) the comment is on. */
    source?: string;
}

export namespace PatchAnchor {
    export function describe(anchor: PatchAnchor): string {
        switch (anchor.type) {
            case 'patch': return 'patch';
            case 'code-file': return anchor.path ?? 'file';
            case 'code-line': return `${anchor.path}:${anchor.line}${anchor.side === 'old' ? ' (old)' : ''}`;
            case 'code-range': return `${anchor.path}:${anchor.startLine}-${anchor.endLine}${anchor.side === 'old' ? ' (old)' : ''}`;
        }
    }
}

/**
 * A review's overview page, its front page (written by Co-Review under its home directory):
 * `…/pages/<reviewId>/overview.md`, or `…/findings/<reviewId>/findings.md` as it was called before.
 */
export function isOverviewPage(uri: string | undefined): boolean {
    return !!uri && /\/(pages\/[^/]+\/overview|findings\/[^/]+\/findings)\.md$/.test(uri);
}

export namespace DocAnchor {
    export function describe(anchor: DocAnchor): string {
        switch (anchor.type) {
            case 'document': return 'document';
            case 'text': return `“${(anchor.exact ?? '').slice(0, 60)}${(anchor.exact ?? '').length > 60 ? '…' : ''}”`;
            case 'mermaid-block': return `diagram ${anchor.blockId}`;
            case 'mermaid-node': return `diagram ${anchor.blockId} › ${anchor.nodeId}`;
            case 'mermaid-edge': return `diagram ${anchor.blockId} › ${anchor.edgeId}`;
            case 'tree-node': return `step “${(anchor.nodeId ?? '').slice(0, 50)}”`;
        }
    }
}

/**
 * Content anchor captured when a comment is created. Used to re-locate the comment when
 * the code around it moves, so that line numbers are never the only location mechanism.
 */
export interface CodeAnchor {
    /** The exact text covered by the range at creation time. */
    text: string;
    /** The line preceding the range (for disambiguation). */
    before?: string;
    /** The line following the range (for disambiguation). */
    after?: string;
    /** Syntax-aware anchor (Tree-sitter), robust against reformatting and moves. */
    syntax?: SyntaxAnchor;
}

export interface SyntaxAnchor {
    /** Tree-sitter grammar the tokens were produced with. */
    grammar: string;
    /** Leaf tokens covered by the range, excluding comments. */
    tokens: string[];
}

export interface CodeLocation {
    kind: LocationKind;
    /** Absent for repository-level comments. */
    uri?: string;
    range?: Range;
    /** Fully qualified symbol path, e.g. `OrderService.CreateOrder`. */
    symbol?: string;
    anchor?: CodeAnchor;
    /** For `kind: 'document'`: where in the rendered document. */
    docAnchor?: DocAnchor;
    /** For `kind: 'patch'`: where in the patch (`uri` is the .patch file). */
    patchAnchor?: PatchAnchor;
}

export type ParticipantKind = 'human' | 'agent' | 'system';

export interface Participant {
    id: string;
    kind: ParticipantKind;
    name: string;
}

export type ToolStatus = 'pending' | 'in_progress' | 'completed' | 'failed';

/** One step of agent work (an ACP tool call), shown as a concise activity line. */
export interface AgentActivity {
    id: string;
    title: string;
    kind?: string;
    status: ToolStatus;
}

export interface PermissionOption {
    id: string;
    name: string;
    /** ACP kinds: allow_once | allow_always | reject_once | reject_always */
    kind: string;
}

/** The agent needs a human decision before it continues (ACP `session/request_permission`). */
export interface PermissionRequest {
    id: string;
    title: string;
    options: PermissionOption[];
    /** The chosen option id, or `cancelled`. */
    outcome?: string;
}

export type MessageStatus = 'streaming' | 'done' | 'error' | 'cancelled';

export interface ReviewMessage {
    id: string;
    author: Participant;
    body: string;
    createdAt: number;
    /** Agent messages: set while the answer is being produced. */
    status?: MessageStatus;
    activity?: AgentActivity[];
    permission?: PermissionRequest;
    /** The reviewer's choice for an agent's question (`ask_reviewer`): delivered to the agent, but needs no reply. */
    choice?: string;
}

/**
 * Where a thread stands with the agent: `queued`, the reviewer's latest message is for the (MCP) agent and it has not
 * picked it up yet; `working`, the agent has it and is answering; `waiting_for_human`, it asked for a decision.
 */
export type AgentState = 'idle' | 'queued' | 'working' | 'waiting_for_human';

/**
 * The agent participating in a review:
 * - `acp`: Co-Review launches it (`command args`) and talks ACP over stdio;
 * - `mcp`: an agent running in its own harness attached itself through Co-Review's MCP tools.
 */
export interface AgentConfig {
    id: string;
    name: string;
    /** Defaults to `acp` (reviews saved before MCP support). */
    transport?: 'acp' | 'mcp';
    command?: string;
    args?: string[];
    /**
     * ACP session settings the reviewer chose, such as the model or reasoning effort: setting id → value
     * (see AgentSetting). Applied to every session the agent opens for this review.
     */
    settings?: Record<string, string>;
    /** How the chosen settings read, e.g. "Claude Sonnet 5 · high" (shown next to the agent). */
    settingsLabel?: string;
}

/** A choice an ACP agent offers for its sessions (an ACP config option, or its session mode). */
export interface AgentSetting {
    /** The config option id, or `AgentSetting.MODE` for the session mode. */
    id: string;
    name: string;
    /** `model`, `thought_level`, `mode`, … */
    category?: string;
    current: string;
    options: { value: string; name: string; description?: string }[];
}

export namespace AgentSetting {
    export const MODE = '__mode';
}

export namespace AgentConfig {
    export function isAcp(agent: AgentConfig | undefined): agent is AgentConfig & { command: string } {
        return !!agent && (agent.transport ?? 'acp') === 'acp' && !!agent.command;
    }
    export function commandLine(agent: AgentConfig): string {
        return [agent.command ?? '', ...(agent.args ?? [])].join(' ').trim();
    }
    /** Whether two configs run the same agent process (they may differ in settings only). */
    export function sameProcess(a: AgentConfig | undefined, b: AgentConfig | undefined): boolean {
        return !!a && !!b && a.id === b.id && a.transport === b.transport && commandLine(a) === commandLine(b);
    }
}

/** Live status of an MCP-attached agent: `listening` while it waits for the reviewer. */
export interface AgentPresence {
    reviewId: string;
    listening: boolean;
    lastSeen: number;
}

/** `proposed`: an automated finding the reviewer has not accepted yet (Accept → open, Dismiss → resolved). */
export type ThreadStatus = 'open' | 'resolved' | 'proposed';

/** `comment`: feedback to address. `question`: a question for an agent to answer ("Ask Agent"). */
export type ThreadIntent = 'comment' | 'question';

export type Severity = 'low' | 'medium' | 'high';

/** A suggested edit: replace `before` with `after`. */
export interface Proposal {
    before: string;
    after: string;
    /** For patch suggestions: the file and first line. */
    path?: string;
    startLine?: number;
    status: 'pending' | 'accepted' | 'rejected';
}

export interface ThreadOptions {
    intent?: ThreadIntent;
    severity?: Severity;
    status?: ThreadStatus;
    /** `finding` for automated findings. */
    origin?: string;
    /** Stable id of a pre-seeded finding; ingestion is idempotent by it. */
    sourceId?: string;
    labels?: string[];
    proposal?: Proposal;
}

export interface ReviewThread {
    id: string;
    /** Human-friendly, per-review sequence number (`Thread #23`). */
    number: number;
    location: CodeLocation;
    messages: ReviewMessage[];
    status: ThreadStatus;
    intent: ThreadIntent;
    severity?: Severity;
    origin?: string;
    sourceId?: string;
    labels?: string[];
    proposal?: Proposal;
    agentState?: AgentState;
    createdAt: number;
    updatedAt: number;
}

export type ReviewScope =
    | { kind: 'repository' }
    | { kind: 'paths'; uris: string[] }
    | { kind: 'branch'; base: string; head: string }
    | { kind: 'commit'; sha: string };

export type ActivityKind = 'review-created' | 'thread-created' | 'message-added' | 'thread-resolved' | 'thread-reopened' | 'review-submitted';

export interface ReviewActivity {
    kind: ActivityKind;
    actor: Participant;
    threadId?: string;
    at: number;
}

export interface Review {
    id: string;
    title: string;
    /**
     * Files the reviewer marked as viewed: repository-relative path → when. A file changed since then counts as
     * not viewed again (see ReviewCoverage).
     */
    viewed?: Record<string, number>;
    /** Workspace root URI the review belongs to. */
    workspaceRoot: string;
    scope: ReviewScope;
    participants: Participant[];
    threads: ReviewThread[];
    activity: ReviewActivity[];
    nextThreadNumber: number;
    /** The agent participating in this review. */
    agent?: AgentConfig;
    /** A review directory (document + patches) an agent handed over. */
    bundle?: ReviewBundle;
    /** The agent's overview of the change or repository (Markdown), shown on the review's overview page. */
    overview?: string;
    /** The reviewer's latest submission (Submit review). */
    verdict?: ReviewVerdict;
    /**
     * The reviewer closed the review's window (or quit Co-Review) while an MCP agent was on it: every agent session on
     * the review is told once. Cleared when the review is shown again.
     */
    closed?: { at: number };
    /** When the reviewer archived this review; archived reviews are hidden from the active list but not deleted. */
    archivedAt?: number;
    createdAt: number;
    updatedAt: number;
}

export interface ReviewBundle {
    /** Absolute path of the review directory. */
    dir: string;
    /** Where the review state file is written (default `<dir>/review.json`). */
    storePath?: string;
    /** An OpenSpec change directory shown with the document. */
    openspec?: string;
    /** Bumped each time an accepted edit rewrites the document. */
    docVersion?: number;
    /** The last time the review was posted to its GitHub pull request. */
    github?: { url: string; round: number; postedAt: number };
    /**
     * A change review's code (see CodeFolders): the folder its window opens on, so the IDE works on real files: the
     * repository itself for the reviewer's own change, else a worktree of the change's head (`worktree`).
     */
    code?: string;
    /** The repository the change was diffed in. */
    repo?: string;
    /** The change's base and head commits (`head` undefined: the working tree). */
    base?: string;
    head?: string;
    /** Co-Review created `code` as a git worktree, and removes it with the review. */
    worktree?: boolean;
    /** How much of the worktree is checked out: the change's neighbourhood first, then all of it. */
    checkout?: 'neighbourhood' | 'full';
    /** Files the change affects without changing them (its blast radius, from the Graphify graph): open these next. */
    related?: string[];
}

export type ReviewDecision = 'approve' | 'request-changes' | 'comment';

export interface ReviewVerdict {
    decision: ReviewDecision;
    summary: string;
    submittedAt: number;
    /** Number of submissions so far (the review round). */
    count: number;
    /** For a review with an MCP agent: `waiting` until an agent picks the round up, then `delivered`. */
    toAgent?: 'waiting' | 'delivered';
}

/**
 * Whether the reviewer's latest message in `thread` is waiting for the MCP agent `agentId`: a question to the review's
 * agent seat, anything in a review directory (the agent's own work), or a follow-up in a thread the agent is part of.
 * The backend delivers these to the agent; the frontend shows them as waiting for it.
 */
export function waitsForAgent(review: Review, thread: ReviewThread, agentId: string): boolean {
    const last = thread.messages[thread.messages.length - 1];
    if (thread.status === 'resolved' || !last || last.author.kind !== 'human') {
        return false;
    }
    const seated = review.agent?.transport === 'mcp' && review.agent.id === agentId;
    return ((thread.intent === 'question' || !!review.bundle) && seated) || thread.messages.some(m => m.author.id === agentId);
}

export namespace ReviewScope {
    export function label(scope: ReviewScope, relative: (uri: string) => string = u => u): string {
        switch (scope.kind) {
            case 'repository': return 'Entire repository';
            case 'paths': return scope.uris.map(relative).join(', ');
            case 'branch': return `${scope.base}...${scope.head}`;
            case 'commit': return `Commit ${scope.sha.slice(0, 10)}`;
        }
    }
}

/** How much of the repository the reviewer has looked at: its source files, overall and per area (folder). */
export interface ReviewCoverage {
    total: number;
    viewed: number;
    areas: {
        /** Repository-relative folder, or `.` for files at the root. */
        path: string;
        total: number;
        viewed: number;
        /** The first file of the area not viewed yet (or changed since). */
        next?: string;
    }[];
}
