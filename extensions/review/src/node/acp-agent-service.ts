import { FileUri } from '@theia/core/lib/common/file-uri';
import { inject, injectable, postConstruct } from '@theia/core/shared/inversify';
import type * as acp from '@agentclientprotocol/sdk';
import { ChildProcess, execFile, spawn } from 'child_process';
import { randomUUID } from 'crypto';
import { promises as fs } from 'fs';
import * as path from 'path';
import { Readable, Writable } from 'stream';
import { AgentActivity, AgentConfig, AgentSetting, MessageStatus, Participant, PermissionRequest, Review, ReviewThread } from '../common/review-model';
import { ReviewStore } from './review-store';
import { HumanDecisions } from './agent-coordination';
import { ANSWER_STYLE } from './answer-style';
import { RepoIndex } from './repo-index';

/**
 * Agents known to speak ACP over stdio. A preset is offered only when its command — and the
 * agent CLI it wraps (`requires`) — is on the PATH.
 */
export const AGENT_PRESETS: (AgentConfig & { command: string; requires?: string })[] = [
    { id: 'claude', name: 'Claude Code', transport: 'acp', command: 'npx', args: ['-y', '@agentclientprotocol/claude-agent-acp'] },
    { id: 'gemini', name: 'Gemini CLI', transport: 'acp', command: 'gemini', args: ['--experimental-acp'] },
    { id: 'opencode', name: 'OpenCode', transport: 'acp', command: 'opencode', args: ['acp'] },
    { id: 'goose', name: 'Goose', transport: 'acp', command: 'goose', args: ['acp'] },
    { id: 'omp', name: 'Oh My Pi', transport: 'acp', command: 'omp', args: ['acp'] },
    { id: 'pi', name: 'Pi', transport: 'acp', command: 'npx', args: ['-y', 'pi-acp'], requires: 'pi' },
    { id: 'codex', name: 'Codex', transport: 'acp', command: 'npx', args: ['-y', '@zed-industries/codex-acp'], requires: 'codex' }
];

/** How long a first question waits for the repository map before it is sent without one. */
const MAP_WAIT_MS = 2000;
/** Budget for the map in the first prompt; the agent reads files for anything beyond it. */
const MAP_CHARS = 8000;

/** State of one prompt turn: the agent message being written into a thread. */
interface Turn {
    reviewId: string;
    threadId: string;
    messageId: string;
    body: string;
    activity: AgentActivity[];
    permission?: PermissionRequest;
    flushTimer?: NodeJS.Timeout;
    /** The agent message being streamed; a turn can hold several (e.g. "Let me look…", then the answer). */
    agentMessageId?: string;
}

interface AgentConnection {
    config: AgentConfig;
    process: ChildProcess;
    acp: acp.ClientSideConnection;
    ready: Promise<acp.InitializeResponse>;
    /** threadId -> ACP session id; one session per thread keeps follow-ups in context. */
    sessions: Map<string, string>;
    /** sessionId -> running turn */
    turns: Map<string, Turn>;
    /**
     * A session opened ahead of the next question (and to learn the agent's settings), so a new thread
     * does not wait for `session/new`.
     */
    spare?: Promise<acp.NewSessionResponse>;
    /** sessionId -> the session's config options, as the agent last reported them */
    options: Map<string, acp.SessionConfigOption[]>;
    stderr: string[];
}

function agentParticipant(config: AgentConfig): Participant {
    return { id: `agent:${config.id}`, kind: 'agent', name: config.name };
}

function which(command: string): Promise<boolean> {
    return new Promise(resolve => execFile('/usr/bin/env', ['which', command], err => resolve(!err)));
}

/**
 * Connects reviews to ACP agents. Co-Review is the ACP *client*: it launches the agent,
 * sends it the questions asked in review threads, and streams its answers, tool activity and
 * permission requests back into the same threads.
 */
@injectable()
export class AcpAgentService {

    @inject(ReviewStore) protected readonly store: ReviewStore;
    @inject(HumanDecisions) protected readonly decisions: HumanDecisions;
    @inject(RepoIndex) protected readonly index: RepoIndex;

    /** reviewId -> connection */
    protected readonly connections = new Map<string, AgentConnection>();
    /** threadId -> tail of the queue of turns for that thread */
    protected readonly queues = new Map<string, Promise<void>>();

    protected sdk: Promise<typeof import('@agentclientprotocol/sdk')> | undefined;

    @postConstruct()
    protected init(): void {
        this.store.onDidAddMessage(({ review, thread, message }) => {
            if (message.author.kind !== 'human' || !AgentConfig.isAcp(review.agent)) {
                return;
            }
            // Questions go to the review's agent; so does any follow-up in a thread this agent is part of.
            const self = agentParticipant(review.agent).id;
            const engaged = thread.intent === 'question' || thread.messages.some(m => m.author.id === self);
            if (engaged) {
                this.ask(review.id, thread.id);
            }
        });
        this.store.onDidChange(change => {
            if (change.kind === 'deleted') {
                this.disconnect(change.reviewId);
            } else {
                const connection = this.connections.get(change.review.id);
                if (connection && !AgentConfig.sameProcess(connection.config, change.review.agent)) {
                    this.disconnect(change.review.id);
                } else if (connection && JSON.stringify(connection.config.settings) !== JSON.stringify(change.review.agent?.settings)) {
                    // Only the settings changed (e.g. another model): apply them to the open sessions too.
                    connection.config = change.review.agent!;
                    for (const sessionId of connection.sessions.values()) {
                        this.applySettings(connection, sessionId).catch(e => console.error('[co-review] agent settings failed', e));
                    }
                }
                if (AgentConfig.isAcp(change.review.agent) && !this.connections.has(change.review.id)) {
                    this.warm(change.review);
                }
            }
        });
        process.once('exit', () => [...this.connections.keys()].forEach(id => this.disconnect(id)));
    }

    async getPresets(): Promise<AgentConfig[]> {
        const available = await Promise.all(AGENT_PRESETS.map(async p => await which(p.command) && (!p.requires || await which(p.requires))));
        return AGENT_PRESETS.filter((_, i) => available[i]).map(({ requires, ...config }) => config);
    }

    /**
     * Starts the agent and the repository map as soon as an agent is connected, so the first question
     * does not wait for the agent to launch (`npx` alone can take seconds) or the map to build.
     */
    protected warm(review: Review): void {
        this.connect(review).then(connection => this.spareSession(connection, review)).catch(() => undefined);
        this.index.warm(FileUri.fsPath(review.workspaceRoot));
    }

    /** The choices the review's agent offers for its sessions: its config options (model, effort, …) and modes. */
    async getSettings(reviewId: string): Promise<AgentSetting[]> {
        const review = await this.store.get(reviewId);
        if (!AgentConfig.isAcp(review?.agent)) {
            return [];
        }
        const connection = await this.connect(review);
        const session = await this.spareSession(connection, review);
        const settings: AgentSetting[] = (session.configOptions ?? [])
            .filter((o): o is Extract<acp.SessionConfigOption, { type: 'select' }> => o.type === 'select').map(o => ({
            id: o.id, name: o.name, category: o.category ?? undefined, current: String(o.currentValue),
            options: (o.options as (acp.SessionConfigSelectOption | acp.SessionConfigSelectGroup)[])
                .flatMap(g => 'options' in g ? g.options : [g])
                .map(v => ({ value: v.value, name: v.name, description: v.description ?? undefined }))
        }));
        const modes = session.modes;
        if (modes?.availableModes.length && !settings.some(s => s.category === 'mode')) {
            settings.push({
                id: AgentSetting.MODE, name: 'Mode', category: 'mode', current: modes.currentModeId,
                options: modes.availableModes.map(m => ({ value: m.id, name: m.name, description: m.description ?? undefined }))
            });
        }
        return settings;
    }

    /** The spare session, opened now if there is none. */
    protected spareSession(connection: AgentConnection, review: Review): Promise<acp.NewSessionResponse> {
        if (!connection.spare) {
            const spare = connection.ready.then(() =>
                connection.acp.newSession({ cwd: FileUri.fsPath(review.workspaceRoot), mcpServers: [] })).then(session => {
                connection.options.set(session.sessionId, session.configOptions ?? []);
                return session;
            });
            // A failed session/new is retried by the next caller.
            spare.catch(() => connection.spare === spare && (connection.spare = undefined));
            connection.spare = spare;
        }
        return connection.spare;
    }

    /** Applies the reviewer's chosen settings (model, effort, mode) to a session, where they differ. */
    protected async applySettings(connection: AgentConnection, sessionId: string): Promise<void> {
        for (const [id, value] of Object.entries(connection.config.settings ?? {})) {
            if (id === AgentSetting.MODE) {
                await connection.acp.setSessionMode({ sessionId, modeId: value });
                continue;
            }
            const option = connection.options.get(sessionId)?.find(o => o.id === id);
            if (option && String(option.currentValue) !== value) {
                const response = await connection.acp.setSessionConfigOption({ sessionId, configId: id, value });
                connection.options.set(sessionId, response.configOptions);
            }
        }
    }

    /** Queues a turn for the thread: the agent answers the latest human messages. */
    ask(reviewId: string, threadId: string): Promise<void> {
        const previous = this.queues.get(threadId) ?? Promise.resolve();
        const next = previous.then(() => this.runTurn(reviewId, threadId)).catch(e => console.error('[co-review] agent turn failed', e));
        this.queues.set(threadId, next);
        return next;
    }

    async cancel(reviewId: string, threadId: string): Promise<void> {
        const connection = this.connections.get(reviewId);
        const sessionId = connection?.sessions.get(threadId);
        if (!connection || !sessionId) {
            return;
        }
        const turn = connection.turns.get(sessionId);
        if (turn?.permission && !turn.permission.outcome) {
            this.decisions.resolve(turn.permission.id, undefined);
        }
        await connection.acp.cancel({ sessionId });
    }

    protected async runTurn(reviewId: string, threadId: string): Promise<void> {
        const review = await this.store.get(reviewId);
        const thread = review?.threads.find(t => t.id === threadId);
        if (!AgentConfig.isAcp(review?.agent) || !thread) {
            return;
        }
        const author = agentParticipant(review.agent);
        const messageId = await this.store.startMessage(reviewId, threadId, author, { status: 'streaming', activity: [] });
        await this.store.setAgentState(reviewId, threadId, 'working');
        const turn: Turn = { reviewId, threadId, messageId, body: '', activity: [] };
        let status: MessageStatus = 'done';
        try {
            const connection = await this.connect(review);
            await connection.ready;
            let sessionId = connection.sessions.get(threadId);
            const fresh = !sessionId;
            if (!sessionId) {
                // Take the spare session, and open the next one in the background.
                sessionId = (await this.spareSession(connection, review)).sessionId;
                connection.spare = undefined;
                connection.sessions.set(threadId, sessionId);
                this.spareSession(connection, review).catch(() => undefined);
                await this.applySettings(connection, sessionId).catch(e => console.error('[co-review] agent settings failed', e));
            }
            connection.turns.set(sessionId, turn);
            try {
                const response = await connection.acp.prompt({ sessionId, prompt: await this.buildPrompt(review, thread, fresh) });
                status = response.stopReason === 'cancelled' ? 'cancelled' : 'done';
                if (response.stopReason === 'refusal') {
                    turn.body += turn.body ? '\n\n_(The agent declined to continue.)_' : '_The agent declined to answer._';
                }
            } finally {
                connection.turns.delete(sessionId);
            }
        } catch (error) {
            status = 'error';
            const connection = this.connections.get(reviewId);
            const details = connection?.stderr.slice(-5).join('').trim();
            turn.body += `${turn.body ? '\n\n' : ''}**Agent error:** ${error instanceof Error ? error.message : String(error)}`
                + (details ? `\n\n\`\`\`\n${details.slice(-1500)}\n\`\`\`` : '');
        }
        clearTimeout(turn.flushTimer);
        if (!turn.body.trim() && status === 'done') {
            turn.body = '_(No answer.)_';
        }
        await this.store.updateMessage(reviewId, threadId, messageId, { body: turn.body, activity: turn.activity, permission: turn.permission, status });
        await this.store.setAgentState(reviewId, threadId, 'idle');
    }

    /** The thread as context: location, code, and the conversation (all of it for a new session). */
    protected async buildPrompt(review: Review, thread: ReviewThread, fresh: boolean): Promise<acp.ContentBlock[]> {
        const root = FileUri.fsPath(review.workspaceRoot);
        const location = thread.location;
        const file = location.uri ? FileUri.fsPath(location.uri) : undefined;
        const relative = file ? path.relative(root, file) : undefined;
        const lines = location.range ? `${location.range.start.line + 1}-${location.range.end.line + 1}` : undefined;
        const human = thread.messages.filter(m => m.author.kind === 'human');
        const blocks: acp.ContentBlock[] = [];
        if (fresh) {
            const where = location.kind === 'repository' || !relative ? 'the repository as a whole'
                : `${relative}${lines ? `:${lines}` : ''}${location.symbol ? ` (${location.symbol})` : ''}`;
            const intro = [
                `You are a participant in a code review of the repository at ${root} ("${review.title}").`,
                `A reviewer asked you about ${where} in review thread #${thread.number}.`,
                'Look at the code you need, then answer in the thread. A map of the repository follows; use it to go straight to the right files.',
                'Do not modify files unless the reviewer explicitly asks you to change code.',
                '',
                ANSWER_STYLE
            ];
            blocks.push({ type: 'text', text: intro.join('\n') });
            const map = await this.repoMap(root, relative);
            if (map) {
                blocks.push({ type: 'text', text: map });
            }
            if (location.anchor?.text && location.kind !== 'repository') {
                blocks.push({ type: 'text', text: `The code in question${lines ? ` (lines ${lines})` : ''}:\n\`\`\`\n${location.anchor.text}\n\`\`\`` });
            }
            if (file) {
                blocks.push({ type: 'resource_link', uri: FileUri.create(file).toString(), name: relative ?? path.basename(file) });
            }
            const history = thread.messages.filter(m => m.status !== 'streaming' && m.body.trim())
                .map(m => `${m.author.kind === 'agent' ? 'Agent' : `Reviewer (${m.author.name})`}: ${m.body}`);
            blocks.push({ type: 'text', text: `Conversation:\n\n${history.join('\n\n')}` });
        } else {
            // The session already has the context; send what the reviewer said since the last answer.
            const lastAgent = thread.messages.map(m => m.author.kind).lastIndexOf('agent');
            const since = thread.messages.slice(lastAgent + 1).filter(m => m.author.kind === 'human');
            blocks.push({ type: 'text', text: (since.length ? since : human.slice(-1)).map(m => m.body).join('\n\n') });
        }
        return blocks;
    }

    /** The repository map for a first prompt, files near the question first; skipped if it is not ready in time. */
    protected async repoMap(root: string, focus: string | undefined): Promise<string | undefined> {
        let timer: NodeJS.Timeout | undefined;
        const timeout = new Promise<undefined>(resolve => {
            timer = setTimeout(() => resolve(undefined), MAP_WAIT_MS);
        });
        const map = this.index.render(root, { focus, maxChars: MAP_CHARS }).catch(error => {
            console.error('[co-review] repository map failed', error);
            return undefined;
        });
        try {
            return await Promise.race([map, timeout]);
        } finally {
            clearTimeout(timer);
        }
    }

    protected async connect(review: Review): Promise<AgentConnection> {
        const existing = this.connections.get(review.id);
        if (existing && existing.process.exitCode === null && !existing.process.killed) {
            return existing;
        }
        const config = review.agent as AgentConfig & { command: string };
        const sdk = await (this.sdk ??= import('@agentclientprotocol/sdk'));
        const root = FileUri.fsPath(review.workspaceRoot);
        const child = spawn(config.command, config.args ?? [], { cwd: root, env: process.env, stdio: ['pipe', 'pipe', 'pipe'] });
        const stderr: string[] = [];
        child.stderr!.on('data', chunk => {
            stderr.push(chunk.toString());
            stderr.splice(0, Math.max(0, stderr.length - 50));
        });
        const spawned = new Promise<void>((resolve, reject) => {
            child.once('spawn', resolve);
            child.once('error', e => reject(new Error(`Could not start ${config.name} (\`${AgentConfig.commandLine(config)}\`): ${e.message}`)));
        });
        const stream = sdk.ndJsonStream(Writable.toWeb(child.stdin!) as WritableStream<Uint8Array>, Readable.toWeb(child.stdout as unknown as Parameters<typeof Readable.toWeb>[0]) as unknown as ReadableStream<Uint8Array>);
        const connection: AgentConnection = {
            config, process: child, stderr,
            sessions: new Map(), turns: new Map(), options: new Map(),
            acp: undefined!, ready: undefined!
        };
        connection.acp = new sdk.ClientSideConnection(() => this.createClient(connection, root), stream);
        connection.ready = spawned.then(() => connection.acp.initialize({
            protocolVersion: sdk.PROTOCOL_VERSION,
            clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } },
            clientInfo: { name: 'co-review', version: '0.1.0' }
        }));
        // A command that could not start never exits; forget it so the next question tries again.
        child.once('error', () => this.connections.get(review.id) === connection && this.connections.delete(review.id));
        child.once('exit', code => {
            if (this.connections.get(review.id) === connection) {
                this.connections.delete(review.id);
            }
            for (const turn of connection.turns.values()) {
                turn.body += `\n\n**The agent process exited${code !== null ? ` (code ${code})` : ''}.**`;
            }
        });
        this.connections.set(review.id, connection);
        return connection;
    }

    protected disconnect(reviewId: string): void {
        const connection = this.connections.get(reviewId);
        this.connections.delete(reviewId);
        connection?.process.kill();
    }

    protected createClient(connection: AgentConnection, root: string): acp.Client {
        const inRoot = (file: string) => {
            const resolved = path.resolve(root, file);
            if (resolved !== root && !resolved.startsWith(root + path.sep)) {
                throw new Error(`Access outside the repository is not allowed: ${file}`);
            }
            return resolved;
        };
        return {
            sessionUpdate: async ({ sessionId, update }) => {
                const turn = connection.turns.get(sessionId);
                if (!turn) {
                    return;
                }
                switch (update.sessionUpdate) {
                    case 'agent_message_chunk':
                        if (update.content.type === 'text') {
                            // A new message in the same turn: the thread shows the last one as the answer, and
                            // earlier ones ("Let me check…", a first draft) become steps, so answers stay short.
                            const id = update.messageId ?? undefined;
                            if (id && turn.agentMessageId && id !== turn.agentMessageId && turn.body.trim()) {
                                const text = turn.body.trim().replace(/\s+/g, ' ');
                                turn.activity.push({ id: `message:${turn.agentMessageId}`, title: text.length > 120 ? `${text.slice(0, 117)}…` : text, kind: 'think', status: 'completed' });
                                turn.body = '';
                            }
                            turn.agentMessageId = id ?? turn.agentMessageId;
                            turn.body += update.content.text;
                        }
                        break;
                    case 'config_option_update':
                        connection.options.set(sessionId, update.configOptions);
                        break;
                    case 'tool_call':
                        turn.activity.push({ id: update.toolCallId, title: update.title, kind: update.kind, status: update.status ?? 'pending' });
                        break;
                    case 'tool_call_update': {
                        const activity = turn.activity.find(a => a.id === update.toolCallId);
                        if (activity) {
                            activity.title = update.title ?? activity.title;
                            activity.status = update.status ?? activity.status;
                            activity.kind = update.kind ?? activity.kind;
                        } else {
                            turn.activity.push({ id: update.toolCallId, title: update.title ?? 'Tool call', kind: update.kind ?? undefined, status: update.status ?? 'in_progress' });
                        }
                        break;
                    }
                }
                this.scheduleFlush(turn);
            },
            requestPermission: async params => {
                const turn = connection.turns.get(params.sessionId);
                if (!turn) {
                    return { outcome: { outcome: 'cancelled' } };
                }
                const request: PermissionRequest = {
                    id: randomUUID(),
                    title: params.toolCall.title ?? 'The agent wants to run a tool',
                    options: params.options.map(o => ({ id: o.optionId, name: o.name, kind: o.kind }))
                };
                turn.permission = request;
                const answer = this.decisions.wait(request.id);
                await this.flush(turn);
                await this.store.setAgentState(turn.reviewId, turn.threadId, 'waiting_for_human');
                const optionId = await answer;
                request.outcome = optionId ?? 'cancelled';
                await this.flush(turn);
                await this.store.setAgentState(turn.reviewId, turn.threadId, 'working');
                return optionId ? { outcome: { outcome: 'selected', optionId } } : { outcome: { outcome: 'cancelled' } };
            },
            readTextFile: async ({ path: file, line, limit }) => {
                const content = await fs.readFile(inRoot(file), 'utf8');
                if (line === undefined && limit === undefined) {
                    return { content };
                }
                const start = Math.max(0, (line ?? 1) - 1);
                return { content: content.split('\n').slice(start, limit ? start + limit : undefined).join('\n') };
            },
            writeTextFile: async ({ path: file, content }) => {
                await fs.writeFile(inRoot(file), content, 'utf8');
                return {};
            }
        };
    }

    protected scheduleFlush(turn: Turn): void {
        if (!turn.flushTimer) {
            turn.flushTimer = setTimeout(() => {
                turn.flushTimer = undefined;
                this.flush(turn);
            }, 250);
        }
    }

    protected flush(turn: Turn): Promise<unknown> {
        return this.store.updateMessage(turn.reviewId, turn.threadId, turn.messageId, {
            body: turn.body, activity: [...turn.activity], permission: turn.permission && { ...turn.permission }
        }).catch(() => undefined);
    }
}
