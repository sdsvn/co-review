import { Emitter } from '@theia/core/lib/common/event';
import { injectable } from '@theia/core/shared/inversify';
import { AgentPresence } from '../common/review-model';

/**
 * Decisions an agent is waiting for (ACP permission requests, MCP `ask_reviewer` questions).
 * The reviewer answers them from the thread; whoever asked is resumed.
 */
@injectable()
export class HumanDecisions {

    protected readonly pending = new Map<string, (optionId: string | undefined) => void>();

    /** Resolves with the chosen option id, or `undefined` when cancelled / timed out. */
    wait(requestId: string, timeoutMs?: number): Promise<string | undefined> {
        return new Promise(resolve => {
            const timer = timeoutMs ? setTimeout(() => this.resolve(requestId, undefined), timeoutMs) : undefined;
            this.pending.set(requestId, optionId => {
                clearTimeout(timer);
                resolve(optionId);
            });
        });
    }

    resolve(requestId: string, optionId: string | undefined): void {
        const resolver = this.pending.get(requestId);
        this.pending.delete(requestId);
        resolver?.(optionId);
    }
}

/** Whether an MCP-attached agent is currently waiting for the reviewer (per review). */
@injectable()
export class AgentPresenceTracker {

    protected readonly presence = new Map<string, AgentPresence & { waiters: number }>();
    protected readonly onDidChangeEmitter = new Emitter<AgentPresence>();
    readonly onDidChange = this.onDidChangeEmitter.event;

    get(reviewId: string): AgentPresence | undefined {
        const p = this.presence.get(reviewId);
        return p && { reviewId, listening: p.listening, lastSeen: p.lastSeen };
    }

    /** Marks the agent as listening until the returned function is called. */
    listen(reviewId: string): () => void {
        const p = this.presence.get(reviewId) ?? { reviewId, listening: false, lastSeen: 0, waiters: 0 };
        p.waiters++;
        this.presence.set(reviewId, p);
        this.update(p);
        return () => {
            p.waiters = Math.max(0, p.waiters - 1);
            this.update(p);
        };
    }

    seen(reviewId: string): void {
        const p = this.presence.get(reviewId) ?? { reviewId, listening: false, lastSeen: 0, waiters: 0 };
        this.presence.set(reviewId, p);
        this.update(p);
    }

    protected update(p: AgentPresence & { waiters: number }): void {
        p.listening = p.waiters > 0;
        p.lastSeen = Date.now();
        this.onDidChangeEmitter.fire({ reviewId: p.reviewId, listening: p.listening, lastSeen: p.lastSeen });
    }
}

/** Who closed a review's window: the reviewer, or Co-Review for an agent that was done with it. */
export type WindowCloser = 'reviewer' | 'agent';

/**
 * The review windows (frontends) open per workspace, so closing one can be told to the agents in that review, and
 * the windows agents showed, so they close when those agents are done.
 *
 * - A frontend says which workspace it shows (`attach`); when its connection goes and no other frontend shows that
 *   workspace for a few seconds (a reload reconnects well within that), the window is closed. When the app quits,
 *   `shutdown` closes them all at once.
 * - In an app an agent started (`--background`, from `co-review mcp`), a window an agent showed with `open_review`
 *   is asked to close when the last agent using it is done (the reviewer approved, or its session ended). The window
 *   says so and closes after a countdown, unless the reviewer keeps it open. With no window left the app quits, as it
 *   would for the reviewer. A window the reviewer opened, or any window of an app they started, stays.
 */
@injectable()
export class ReviewWindows {

    protected readonly startedByAgent = !!process.versions.electron && process.argv.includes('--background');
    /** Workspace URI -> the agent sessions that showed it. */
    protected readonly shownBy = new Map<string, Set<string>>();
    /** Workspace URI -> the frontends showing it. */
    protected readonly frontends = new Map<string, Set<object>>();
    /** Workspaces whose window Co-Review asked to close (for an agent), with when it asked. */
    protected readonly closingForAgent = new Map<string, number>();

    protected readonly onDidRequestCloseEmitter = new Emitter<{ workspaceRoot: string; message: string }>();
    /** A window should close: the agents that showed it are done. `message` says why, for the reviewer. */
    readonly onDidRequestClose = this.onDidRequestCloseEmitter.event;
    protected readonly onDidOpenEmitter = new Emitter<string>();
    /** A workspace got its first window. */
    readonly onDidOpen = this.onDidOpenEmitter.event;
    protected readonly onDidCloseEmitter = new Emitter<{ workspaceRoot: string; by: WindowCloser }>();
    /** The last window of a workspace closed. */
    readonly onDidClose = this.onDidCloseEmitter.event;

    /** A frontend shows `workspaceRoot` (it may have shown another one before). */
    attach(workspaceRoot: string, frontend: object): void {
        this.detach(frontend, false);
        const set = this.frontends.get(workspaceRoot) ?? new Set();
        const first = !set.size;
        set.add(frontend);
        this.frontends.set(workspaceRoot, set);
        if (first) {
            this.onDidOpenEmitter.fire(workspaceRoot);
        }
    }

    /** A frontend went away; its workspace's window counts as closed when none comes back shortly. */
    detach(frontend: object, closing = true): void {
        for (const [root, set] of this.frontends) {
            if (set.delete(frontend) && !set.size && closing) {
                setTimeout(() => !this.frontends.get(root)?.size && this.closed(root), 4000);
            }
        }
    }

    /** The app is quitting: every window is closing. */
    shutdown(): void {
        for (const [root, set] of this.frontends) {
            if (set.size) {
                set.clear();
                this.closed(root);
            }
        }
    }

    protected closed(workspaceRoot: string): void {
        this.frontends.delete(workspaceRoot);
        const asked = this.closingForAgent.get(workspaceRoot);
        this.closingForAgent.delete(workspaceRoot);
        // Closed within the countdown Co-Review started: for the agent. Later (they kept it open): by the reviewer.
        const by: WindowCloser = asked !== undefined && Date.now() - asked < 30_000 ? 'agent' : 'reviewer';
        this.onDidCloseEmitter.fire({ workspaceRoot, by });
    }

    shown(workspaceRoot: string, session: string): void {
        if (this.startedByAgent) {
            const sessions = this.shownBy.get(workspaceRoot) ?? new Set();
            sessions.add(session);
            this.shownBy.set(workspaceRoot, sessions);
        }
    }

    /** `session` is done with the window of `workspaceRoot` (all its windows when omitted); `message` tells the reviewer why. */
    done(session: string, message: string, workspaceRoot?: string): void {
        for (const [root, sessions] of this.shownBy) {
            if ((workspaceRoot === undefined || root === workspaceRoot) && sessions.delete(session) && !sessions.size) {
                this.shownBy.delete(root);
                this.closingForAgent.set(root, Date.now());
                this.onDidRequestCloseEmitter.fire({ workspaceRoot: root, message });
            }
        }
    }
}
