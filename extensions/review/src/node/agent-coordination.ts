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
 * What a window shows: a workspace, and the review it was opened for (`?review=<id>`) when an agent showed it. An
 * agent's review gets a window of its own, so several reviews of one repository are open side by side, and closing
 * one is told only to its agents.
 */
export interface ReviewWindow {
    workspaceRoot: string;
    reviewId?: string;
}

function keyOf(w: ReviewWindow): string {
    return w.reviewId ? `${w.workspaceRoot}?review=${w.reviewId}` : w.workspaceRoot;
}

/**
 * The review windows (frontends) open, so closing one can be told to the agents in its review, and the windows
 * agents showed, so they close when those agents are done.
 *
 * - A frontend says what it shows (`attach`); when its connection goes and no other frontend shows the same for a
 *   few seconds (a reload reconnects well within that), the window is closed. When the app quits, `shutdown` closes
 *   them all at once.
 * - In an app an agent started (`--background`, from `co-review mcp`), a window an agent showed with `open_review`
 *   is asked to close when the last agent using it is done (the reviewer submitted, or its session ended). The window
 *   says so and closes after a countdown, unless the reviewer keeps it open. With no window left the app quits, as it
 *   would for the reviewer. A window the reviewer opened, or any window of an app they started, stays.
 */
@injectable()
export class ReviewWindows {

    protected readonly startedByAgent = !!process.versions.electron && process.argv.includes('--background');
    /** Window key -> what it shows. */
    protected readonly windows = new Map<string, ReviewWindow>();
    /** Window key -> the agent sessions that showed it. */
    protected readonly shownBy = new Map<string, Set<string>>();
    /** Window key -> the frontends showing it. */
    protected readonly frontends = new Map<string, Set<object>>();
    /** Windows Co-Review asked to close (for an agent), with when it asked. */
    protected readonly closingForAgent = new Map<string, number>();

    protected readonly onDidRequestCloseEmitter = new Emitter<ReviewWindow & { message: string }>();
    /** A window should close: the agents that showed it are done. `message` says why, for the reviewer. */
    readonly onDidRequestClose = this.onDidRequestCloseEmitter.event;
    protected readonly onDidOpenEmitter = new Emitter<ReviewWindow>();
    /** A window got its first frontend. */
    readonly onDidOpen = this.onDidOpenEmitter.event;
    protected readonly onDidCloseEmitter = new Emitter<ReviewWindow & { by: WindowCloser }>();
    /** The last frontend of a window closed. */
    readonly onDidClose = this.onDidCloseEmitter.event;

    /** Windows whose last frontend went, waiting a few seconds for a reload to come back before counting as closed. */
    protected readonly closing = new Map<string, NodeJS.Timeout>();

    /** A frontend shows `window` (it may have shown another one before). */
    attach(window: ReviewWindow, frontend: object): void {
        this.detach(frontend);
        const key = keyOf(window);
        this.windows.set(key, { workspaceRoot: window.workspaceRoot, reviewId: window.reviewId });
        const set = this.frontends.get(key) ?? new Set();
        // Back within the grace period (a reload): not closed, and not newly opened either.
        const reloaded = this.closing.has(key);
        clearTimeout(this.closing.get(key));
        this.closing.delete(key);
        const first = !set.size && !reloaded;
        set.add(frontend);
        this.frontends.set(key, set);
        if (first) {
            this.onDidOpenEmitter.fire(this.windows.get(key)!);
        }
    }

    /** A frontend went away; its window counts as closed when none comes back shortly. */
    detach(frontend: object): void {
        for (const [key, set] of this.frontends) {
            if (set.delete(frontend) && !set.size) {
                clearTimeout(this.closing.get(key));
                this.closing.set(key, setTimeout(() => this.closed(key), 4000));
            }
        }
    }

    /** Whether a window opened for `reviewId` is open (or reloading). */
    hasWindowOf(reviewId: string): boolean {
        return [...this.frontends.keys()].some(key => this.windows.get(key)?.reviewId === reviewId);
    }

    /** The reviewer kept open a window Co-Review was closing for an agent: closing it later is theirs. */
    keptOpen(window: ReviewWindow): void {
        this.closingForAgent.delete(keyOf(window));
    }

    /** The app is quitting: every window is closing, also those whose frontend already went (quitting closes them first). */
    shutdown(): void {
        for (const key of new Set([...this.frontends.keys(), ...this.closing.keys()])) {
            this.closed(key);
        }
    }

    protected closed(key: string): void {
        clearTimeout(this.closing.get(key));
        this.closing.delete(key);
        if (!this.frontends.delete(key)) {
            return;
        }
        const asked = this.closingForAgent.get(key);
        this.closingForAgent.delete(key);
        // Closed within the countdown Co-Review started: for the agent; otherwise by the reviewer.
        const by: WindowCloser = asked !== undefined && Date.now() - asked < 30_000 ? 'agent' : 'reviewer';
        this.onDidCloseEmitter.fire({ ...this.windows.get(key)!, by });
    }

    shown(window: ReviewWindow, session: string): void {
        if (this.startedByAgent) {
            const key = keyOf(window);
            this.windows.set(key, { workspaceRoot: window.workspaceRoot, reviewId: window.reviewId });
            const sessions = this.shownBy.get(key) ?? new Set();
            sessions.add(session);
            this.shownBy.set(key, sessions);
        }
    }

    /** `session` is done with the window of `reviewId` (all its windows when omitted); `message` tells the reviewer why. */
    done(session: string, message: string, reviewId?: string): void {
        for (const [key, sessions] of this.shownBy) {
            const window = this.windows.get(key)!;
            if ((reviewId === undefined || window.reviewId === reviewId) && sessions.delete(session) && !sessions.size) {
                this.shownBy.delete(key);
                this.closingForAgent.set(key, Date.now());
                this.onDidRequestCloseEmitter.fire({ ...window, message });
            }
        }
    }
}
