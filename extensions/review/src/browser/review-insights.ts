import { AgentPresence, Review, ReviewThread, Severity } from '../common/review-model';
import { formatTime } from './review-components';

/** What the review UI derives from a review to guide the reviewer: titles, order, the agent's state and activity. */

const RANK: Record<Severity, number> = { high: 3, medium: 2, low: 1 };

export function severityRank(severity: Severity | undefined): number {
    return severity ? RANK[severity] ?? 0 : 0;
}

/** Markdown emphasis, code ticks and links as plain text (underscores inside words, as in `open_review`, stay). */
function plain(text: string): string {
    return text.replace(/^#+\s*/, '').replace(/\*\*|__|`/g, '').replace(/(^|\s)[*_](\S)/g, '$1$2').replace(/(\S)[*_](\s|$)/g, '$1$2')
        .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1');
}

/** A finding's first message, split into its first sentence (or line) and the rest. */
function split(thread: ReviewThread): { first: string; rest: string } {
    const body = (thread.messages[0]?.body ?? '').trim();
    const line = body.split('\n').map(l => l.trim()).find(Boolean) ?? '';
    const sentence = line.match(/^(.+?[.!?])(\s|$)/)?.[1] ?? line;
    const rest = body.slice(body.indexOf(sentence) + sentence.length).replace(/\s+/g, ' ').trim();
    return { first: plain(sentence), rest: plain(rest) };
}

/** A finding's title: the first sentence (or line) of its first message, without Markdown, at most about 110 characters. */
export function findingTitle(thread: ReviewThread): string {
    const { first } = split(thread);
    return first.length > 110 ? `${first.slice(0, 107).trimEnd()}…` : first;
}

/** What the first message says after its title (the whole first sentence, when the title cut it short). */
export function findingRest(thread: ReviewThread): string {
    const { first, rest } = split(thread);
    return first.length > 110 ? `…${first.slice(107)} ${rest}`.trim() : rest;
}

/** Whether the card's preview is worth showing: the message says more than its title, or there are replies. */
export function hasMore(thread: ReviewThread): boolean {
    return !!findingRest(thread) || thread.messages.length > 1;
}

/** A location label ("a/b/c.ts:12-14 · …") as the file's name with its line, and its folder, to show dimmed. */
export function splitLocation(label: string): { name: string; dir: string } {
    const [where, ...rest] = label.split(' · ');
    const slash = where.lastIndexOf('/', where.includes(':') ? where.indexOf(':') : where.length);
    const name = slash >= 0 && !where.endsWith('/') ? where.slice(slash + 1) : where;
    const dir = slash >= 0 && !where.endsWith('/') ? where.slice(0, slash + 1) : '';
    return { name: [name, ...rest].join(' · '), dir };
}

/** The finding to start with: the most severe one still to triage, else the most severe open one. */
export function firstFinding(review: Review): ReviewThread | undefined {
    const order = (list: ReviewThread[]) => [...list].sort((a, b) => severityRank(b.severity) - severityRank(a.severity) || a.number - b.number)[0];
    return order(review.threads.filter(t => t.status === 'proposed')) ?? order(review.threads.filter(t => t.status === 'open'));
}

export type AgentState = 'none' | 'listening' | 'working' | 'away' | 'connected';

/** What the review's agent is doing, for its status pill: working on a thread, listening, or away since when. */
export function agentStatus(review: Review, presence: AgentPresence | undefined): { state: AgentState; label: string; title: string } {
    const agent = review.agent;
    if (!agent) {
        return { state: 'none', label: 'No agent', title: 'Connect an agent to ask questions' };
    }
    if (review.threads.some(t => t.agentState === 'working')) {
        return { state: 'working', label: 'working', title: `${agent.name} is working on an answer` };
    }
    if (agent.transport !== 'mcp') {
        return { state: 'connected', label: 'ready', title: `${agent.name} (ACP) starts when you ask it something` };
    }
    if (presence?.listening) {
        return { state: 'listening', label: 'listening', title: `${agent.name} is listening: it gets your comments right away` };
    }
    const since = presence?.lastSeen ? ` since ${formatTime(presence.lastSeen)}` : '';
    return { state: 'away', label: `away${since}`, title: `${agent.name} is busy or gone${since}; comments wait for it, nothing is lost` };
}

export interface ActivityEntry {
    key: string;
    text: string;
    at: number;
    threadId?: string;
}

/**
 * The agent's latest moves, newest first, at most `max`: a burst of findings it added (within two minutes) is one
 * entry ("added 5 findings"), each reply names its thread.
 */
export function agentActivity(review: Review, max = 3): ActivityEntry[] {
    const byAgent = review.activity.filter(a => a.actor.kind === 'agent' && a.kind !== 'review-created');
    const entries: (ActivityEntry & { kind: string; count: number })[] = [];
    for (const a of byAgent) {
        const thread = a.threadId ? review.threads.find(t => t.id === a.threadId) : undefined;
        const last = entries[entries.length - 1];
        if (a.kind === 'thread-created' && last?.kind === 'thread-created' && a.at - last.at < 120_000) {
            last.count++;
            last.at = a.at;
            last.text = `${a.actor.name} added ${last.count} findings`;
            last.threadId = undefined;
            continue;
        }
        // A finding's first message is its body, not a reply.
        if (a.kind === 'message-added' && thread && thread.messages[0]?.author.id === a.actor.id
            && Math.abs(thread.messages[0].createdAt - a.at) < 2000) {
            continue;
        }
        const n = thread ? ` #${thread.number}` : '';
        const text = a.kind === 'thread-created' ? `${a.actor.name} added a finding${n}`
            : a.kind === 'message-added' ? `${a.actor.name} replied in${n || ' a thread'}`
                : a.kind === 'thread-resolved' ? `${a.actor.name} resolved${n}`
                    : a.kind === 'thread-reopened' ? `${a.actor.name} reopened${n}`
                        : `${a.actor.name} submitted`;
        entries.push({ key: `${a.at}:${a.kind}:${a.threadId ?? ''}`, text, at: a.at, threadId: a.threadId, kind: a.kind, count: 1 });
    }
    return entries.slice(-max).reverse().map(({ key, text, at, threadId }) => ({ key, text, at, threadId }));
}

/** "2 min", "1 h", or the time: how long ago, for the activity strip. */
export function ago(at: number, now = Date.now()): string {
    const minutes = Math.floor((now - at) / 60_000);
    return minutes < 1 ? 'now' : minutes < 60 ? `${minutes} min` : minutes < 24 * 60 ? `${Math.floor(minutes / 60)} h` : formatTime(at);
}
