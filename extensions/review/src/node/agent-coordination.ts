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

/**
 * Desktop windows that agents showed with `open_review` in an app an agent started (`--background`, from
 * `co-review mcp`). When the last agent using a window is done with its review (the reviewer approved it, or the
 * agent's session ended), the window closes; with no window left the app quits, as it would for the reviewer.
 * A window the reviewer opened themselves, or any window of an app they started, stays.
 */
@injectable()
export class AgentWindows {

    protected readonly startedByAgent = !!process.versions.electron && process.argv.includes('--background');
    /** Workspace URI -> the agent sessions that showed it. */
    protected readonly shownBy = new Map<string, Set<string>>();
    protected readonly onDidRequestCloseEmitter = new Emitter<string>();
    /** Fired with a workspace URI whose window should close. */
    readonly onDidRequestClose = this.onDidRequestCloseEmitter.event;

    shown(workspaceRoot: string, session: string): void {
        if (this.startedByAgent) {
            const sessions = this.shownBy.get(workspaceRoot) ?? new Set();
            sessions.add(session);
            this.shownBy.set(workspaceRoot, sessions);
        }
    }

    /** `session` is done with the window of `workspaceRoot` (all its windows when omitted). */
    done(session: string, workspaceRoot?: string): void {
        for (const [root, sessions] of this.shownBy) {
            if ((workspaceRoot === undefined || root === workspaceRoot) && sessions.delete(session) && !sessions.size) {
                this.shownBy.delete(root);
                // After the agent's tool result is on its way: quitting the app ends this backend too.
                setTimeout(() => !this.shownBy.has(root) && this.onDidRequestCloseEmitter.fire(root), 2000);
            }
        }
    }
}
