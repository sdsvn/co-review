import { BackendApplicationContribution } from '@theia/core/lib/node/backend-application';
import { FileUri } from '@theia/core/lib/common/file-uri';
import * as express from '@theia/core/shared/express';
import { inject, injectable } from '@theia/core/shared/inversify';
import type { McpServer as McpServerType } from '@modelcontextprotocol/sdk/server/mcp.js';
import type { StreamableHTTPServerTransport as TransportType } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { execFile } from 'child_process';
import * as http from 'http';
import { randomUUID } from 'crypto';
import { promises as fs } from 'fs';
import * as path from 'path';
import { z } from 'zod';
import { CodeLocation, Participant, Review, ReviewThread, Severity } from '../common/review-model';
import { SyntaxSymbol } from '../common/syntax-protocol';
import { AgentPresenceTracker, HumanDecisions } from './agent-coordination';
import { coReviewHome, ReviewStore } from './review-store';
import { SyntaxServiceImpl } from './syntax-service-impl';
import { BundleService } from './bundle-service';
import { documentFormat, DocumentFormat } from '../common/design-format';
import { acceptedSuggestions, commentOf, findThread, threadRef } from './review-payloads';
import { ANSWER_STYLE, ANSWER_STYLE_SHORT, DESIGN_PROMPT } from './prompts.gen';
import { LANGUAGE_BY_EXTENSION, RepoIndex } from './repo-index';

/* eslint-disable @typescript-eslint/no-require-imports */
const { McpServer } = require('@modelcontextprotocol/sdk/server/mcp.js') as typeof import('@modelcontextprotocol/sdk/server/mcp.js');
const { StreamableHTTPServerTransport } = require('@modelcontextprotocol/sdk/server/streamableHttp.js') as typeof import('@modelcontextprotocol/sdk/server/streamableHttp.js');
const { isInitializeRequest } = require('@modelcontextprotocol/sdk/types.js') as typeof import('@modelcontextprotocol/sdk/types.js');

interface Session {
    transport: TransportType;
    server: McpServerType;
    /** Human message ids already handed to this agent, so each question is delivered once. */
    delivered: Set<string>;
    /** Submissions already returned by await_review. */
    consumedSubmit: number;
    /** Pushes review events into a Claude Code session (channels); see startChannel. */
    channel?: { dispose(): void };
}

type ToolResult = { content: { type: 'text'; text: string }[]; isError?: boolean };

function json(value: unknown): ToolResult {
    return { content: [{ type: 'text', text: JSON.stringify(value, undefined, 2) }] };
}

function fail(message: string): ToolResult {
    return { content: [{ type: 'text', text: message }], isError: true };
}

/**
 * MCP endpoint (`/mcp`, Streamable HTTP) for agents that run in their own harness
 * (Claude Code, Codex, Cursor, …). The agent opens a review, then acts as a co-reviewer:
 * it blocks on `await_comment` until the reviewer asks something, answers with `reply`,
 * and can add findings or ask the reviewer to decide. The ACP integration is the other
 * direction: there Co-Review launches the agent itself.
 */
@injectable()
export class CoReviewerMcp implements BackendApplicationContribution {

    @inject(ReviewStore) protected readonly store: ReviewStore;
    @inject(SyntaxServiceImpl) protected readonly syntax: SyntaxServiceImpl;
    @inject(HumanDecisions) protected readonly decisions: HumanDecisions;
    @inject(AgentPresenceTracker) protected readonly presence: AgentPresenceTracker;
    @inject(BundleService) protected readonly bundles: BundleService;
    @inject(RepoIndex) protected readonly index: RepoIndex;

    protected readonly sessions = new Map<string, Session>();
    /** Workspace the backend was started with; the default `root`. */
    protected defaultRoot: string | undefined = process.argv.slice(2).filter(a => !a.startsWith('-')).map(a => path.resolve(a)).pop();

    protected get registryFile(): string {
        return path.join(coReviewHome(), 'server.json');
    }

    /** Records where this instance listens, so `co-review mcp` can find a running app (desktop or browser). */
    async onStart(server: http.Server | import('https').Server): Promise<void> {
        const address = server.address();
        if (address && typeof address === 'object') {
            const host = address.address === '::' || address.address === '0.0.0.0' ? '127.0.0.1' : address.address;
            await fs.mkdir(path.dirname(this.registryFile), { recursive: true });
            await fs.writeFile(this.registryFile, JSON.stringify({ url: `http://${host}:${address.port}`, pid: process.pid }, undefined, 2));
        }
    }

    onStop(): void {
        try {
            // Only remove our own registration.
            const current = JSON.parse(require('fs').readFileSync(this.registryFile, 'utf8'));
            if (current.pid === process.pid) {
                require('fs').unlinkSync(this.registryFile);
            }
        } catch {
            /* nothing registered */
        }
    }

    configure(app: express.Application): void {
        app.all('/mcp', express.json({ limit: '4mb' }), (req, res) => this.handle(req, res).catch(error => {
            console.error('[co-review] MCP request failed', error);
            if (!res.headersSent) {
                res.status(500).json({ jsonrpc: '2.0', error: { code: -32603, message: String(error) }, id: null });
            }
        }));
    }

    protected async handle(req: express.Request, res: express.Response): Promise<void> {
        // Only local clients: the endpoint is unauthenticated.
        const host = (req.headers.host ?? '').replace(/:\d+$/, '');
        // Web pages cannot drive it either: browsers send an Origin, agents do not.
        const origin = req.headers.origin === undefined ? undefined : URL.canParse(req.headers.origin) ? new URL(req.headers.origin).hostname : 'invalid';
        if (![host, ...(origin !== undefined ? [origin] : [])].every(h => ['localhost', '127.0.0.1', '[::1]', '::1'].includes(h))) {
            res.status(403).send('Co-Review MCP only accepts local connections');
            return;
        }
        const sessionId = req.headers['mcp-session-id'] as string | undefined;
        let session = sessionId ? this.sessions.get(sessionId) : undefined;
        if (!session) {
            if (req.method !== 'POST' || !isInitializeRequest(req.body)) {
                res.status(sessionId ? 404 : 400).json({ jsonrpc: '2.0', error: { code: -32000, message: 'No valid MCP session' }, id: null });
                return;
            }
            const root = typeof req.query.root === 'string' ? path.resolve(req.query.root) : this.defaultRoot;
            const baseUrl = `http://${req.headers.host}`;
            const created: Session = {
                delivered: new Set(),
                consumedSubmit: 0,
                server: undefined!,
                transport: new StreamableHTTPServerTransport({
                    sessionIdGenerator: () => randomUUID(),
                    onsessioninitialized: id => { this.sessions.set(id, created); }
                })
            };
            created.server = this.createServer(created, root, baseUrl);
            created.transport.onclose = () => {
                created.channel?.dispose();
                return created.transport.sessionId && this.sessions.delete(created.transport.sessionId);
            };
            await created.server.connect(created.transport);
            session = created;
        }
        await session.transport.handleRequest(req, res, req.body);
    }

    protected agentOf(session: Session): Participant {
        const name = session.server.server.getClientVersion()?.name ?? 'Agent';
        return { id: `agent:mcp:${name}`, kind: 'agent', name };
    }

    protected createServer(session: Session, defaultRoot: string | undefined, baseUrl: string): McpServerType {
        const server = new McpServer({ name: 'co-review', version: '0.1.0' }, {
            // Claude Code channels: review events are pushed into the session (see startChannel).
            capabilities: { experimental: { 'claude/channel': {} } },
            instructions: 'Co-Review is a repository review app. Call open_review, tell the reviewer the URL, then loop: '
                + 'await_comment → investigate the repository → reply. Use add_findings for issues you find, '
                + 'ask_reviewer when you need a decision, and repo_map to find where things live before searching. '
                + 'To review the whole repository: repo_map({ overview: true }) for where to start and how the code clusters, then '
                + 'add_findings with the area as the first label and status "proposed" (a few per area); get_review returns `coverage`, '
                + 'the files the reviewer has viewed per area. A knowledge bundle (e.g. OKF) opens with open_review({ dir }); '
                + 'its pages come back as target "doc:<path>". '
                + 'To have a design reviewed before you implement, follow the design-document rules in open_review\'s description. '
                + 'In Claude Code with channels on, the reviewer\'s questions and submissions also arrive as <channel source="co-review"> '
                + 'messages (thread_id attribute): answer each with reply({ threadId: thread_id }) instead of looping await_comment, '
                + 'and on a submission call await_review (it returns immediately).\n\n' + ANSWER_STYLE
        });
        const rootArg = z.string().optional().describe('Absolute path of the repository (defaults to the one Co-Review was started for)');
        const reviewArg = z.string().optional().describe('Review id (defaults to the review opened with open_review)');
        let currentReviewId: string | undefined;

        const resolveReview = async (reviewId?: string, root?: string): Promise<Review | undefined> => {
            const id = reviewId ?? currentReviewId;
            if (id) {
                return this.store.get(id);
            }
            const workspace = root ?? defaultRoot;
            return workspace ? (await this.store.list(FileUri.create(workspace).toString()))[0] : undefined;
        };

        server.registerTool('open_review', {
            description: 'Open a review and join it as co-reviewer; returns the URL to give the reviewer. Either a repository (`root`, '
                + 'default: the one Co-Review was started for) or a review directory / inline content: `dir` (index.markdown + assets + *.patch), '
                + '`markdown` and/or `patch` strings. A `dir` can also be a knowledge bundle such as OKF (`<repo>/okf`): every Markdown page '
                + 'is a review page, `/x.md` links resolve from the bundle root and `path:line` links open the code. '
                + 'Then loop await_comment → reply, and/or await_review for the reviewer\'s Submit.\n\n'
                + 'Design documents: to have a design reviewed before implementing, write the smallest document that lets the reviewer '
                + 'understand, challenge and approve the change:\n' + DESIGN_PROMPT,
            inputSchema: {
                root: rootArg,
                dir: z.string().optional().describe('Review directory (index.markdown or *.pseudocode.md, *.patch, PR.md, <patch>.comments.json)'),
                markdown: z.string().optional().describe('Inline Markdown document (```mermaid fences render as diagrams)'),
                patch: z.string().optional().describe('Inline unified diff / git patch'),
                patchName: z.string().optional().describe('Slug for the inline patch page (default "change")'),
                storePath: z.string().optional().describe('Where to write the review state file (default <dir>/review.json)'),
                title: z.string().optional(),
                open: z.boolean().optional().describe('Open the review in the browser (default true)'),
                addr: z.string().optional().describe('Ignored'),
                openspec: z.string().optional().describe('OpenSpec change directory (…/changes/<id>) shown with the document')
            }
        }, async ({ root, dir, markdown, patch, patchName, storePath, title, open, openspec }) => {
            const agent = this.agentOf(session);
            let review: Review;
            let workspace: string;
            let extra: Record<string, unknown> = {};
            if (dir || markdown || patch) {
                const opened = await this.bundles.open({ dir, markdown, patch, patchName, title, storePath, openspec, author: agent });
                review = opened.review;
                workspace = opened.dir;
                extra = { hasDoc: !!opened.docFile, patches: opened.patches.length, dir: opened.dir, ...this.formatOf(opened.docFile) };
            } else {
                workspace = root ? path.resolve(root) : defaultRoot!;
                if (!workspace) {
                    return fail('No repository: pass `root` (absolute path), or `dir` / `markdown` / `patch`.');
                }
                const workspaceRoot = FileUri.create(workspace).toString();
                review = (await this.store.list(workspaceRoot))[0];
                if (!review || title) {
                    review = await this.store.create({ workspaceRoot, title: title ?? `Review with ${agent.name}`, scope: { kind: 'repository' }, author: agent });
                }
            }
            review = await this.store.setAgent(review.id, { id: agent.id, name: agent.name, transport: 'mcp' });
            currentReviewId = review.id;
            this.startChannel(session, review, agent);
            session.consumedSubmit = review.verdict?.count ?? 0;
            this.presence.seen(review.id);
            // The desktop app shows the review itself (its frontend switches to it); a browser
            // cannot attach to the Electron backend.
            const desktop = !!process.versions.electron;
            const url = desktop ? undefined : `${baseUrl}/?review=${review.id}#${workspace}`;
            if (url && open !== false) {
                execFile(process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'explorer' : 'xdg-open', [url], () => undefined);
            }
            return json({
                reviewId: review.id, title: review.title, openThreads: review.threads.filter(t => t.status === 'open').length, ...extra,
                ...(url ? { url } : { note: 'Shown in the Co-Review desktop app (open the repository there if it is not open).' }),
                hint: 'Share the URL with the reviewer, then loop await_comment → reply, or call await_review to wait for their Submit.'
            });
        });

        server.registerTool('await_review', {
            description: 'Block until the reviewer clicks Submit, then return the batch: {decision, summary, comments[], acceptedSuggestions[]}. '
                + 'Returns {status:"pending"} on timeout — call again to keep waiting.',
            inputSchema: { reviewId: reviewArg, timeoutSec: z.number().int().min(1).max(3600).optional() }
        }, async ({ reviewId, timeoutSec }, extra) => {
            const initial = await resolveReview(reviewId);
            if (!initial) {
                return fail('No review. Call open_review first.');
            }
            const stopListening = this.presence.listen(initial.id);
            try {
                const deadline = Date.now() + (timeoutSec ?? 300) * 1000;
                for (;;) {
                    const review = await this.store.get(initial.id);
                    if (review?.verdict && review.verdict.count > session.consumedSubmit) {
                        session.consumedSubmit = review.verdict.count;
                        return json(this.batch(review));
                    }
                    if (Date.now() >= deadline || extra.signal.aborted) {
                        return json({ status: 'pending', hint: 'No submit yet within the timeout. Call await_review again to keep waiting.' });
                    }
                    await this.nextChange(initial.id, deadline - Date.now(), extra.signal);
                }
            } finally {
                stopListening();
            }
        });

        server.registerTool('await_comment', {
            description: 'Block until the reviewer asks you something or replies in a thread you are part of (in a review directory: any '
                + 'comment). Returns {status:"comment", comments[] (open threads, needsReply), needsReply, threads[] (the ones to answer, with code '
                + 'and conversation)}; on timeout {status:"pending"} — call again.',
            inputSchema: { reviewId: reviewArg, timeoutSec: z.number().int().min(1).max(3600).optional() }
        }, async ({ reviewId, timeoutSec }, extra) => {
            const initial = await resolveReview(reviewId);
            if (!initial) {
                return fail('No review. Call open_review first.');
            }
            const agent = this.agentOf(session);
            const stopListening = this.presence.listen(initial.id);
            try {
                const deadline = Date.now() + (timeoutSec ?? 240) * 1000;
                for (;;) {
                    const review = await this.store.get(initial.id);
                    const waiting = review ? this.waitingThreads(review, agent, session) : [];
                    if (waiting.length) {
                        for (const thread of waiting) {
                            session.delivered.add(thread.messages[thread.messages.length - 1].id);
                            await this.store.setAgentState(review!.id, thread.id, 'working');
                        }
                        return json(this.commentBatch(review!, waiting));
                    }
                    if (Date.now() >= deadline || extra.signal.aborted) {
                        return json({ status: 'pending' });
                    }
                    await this.nextChange(initial.id, deadline - Date.now(), extra.signal);
                }
            } finally {
                stopListening();
            }
        });

        server.registerTool('reply', {
            description: `Answer in a review thread (markdown). ${ANSWER_STYLE_SHORT} threadId: the thread id or its short id ("t2").`,
            inputSchema: { threadId: z.string(), body: z.string(), resolve: z.boolean().optional(), reviewId: reviewArg }
        }, async ({ threadId, body, resolve, reviewId }) => {
            const review = await resolveReview(reviewId);
            const thread = review && findThread(review, threadId.trim());
            if (!review || !thread) {
                return fail(`No thread "${threadId}".`);
            }
            const agent = this.agentOf(session);
            const updated = await this.store.addMessage(review.id, thread.id, body, agent);
            await this.store.setAgentState(review.id, thread.id, 'idle');
            if (resolve) {
                await this.store.setThreadStatus(review.id, thread.id, 'resolved', agent);
            }
            this.presence.seen(review.id);
            return json({ ok: true, thread: threadRef(updated), threadId: updated.id, messages: updated.messages.length });
        });

        server.registerTool('add_findings', {
            description: 'Add findings. Two shapes: {path, line?, endLine?, body, severity?, labels?, status?} creates threads on repository code '
                + '(without `line`, on the file or folder; `labels` group them in the panel, e.g. the area of the repository; '
                + '`status: "proposed"` lets the reviewer Accept or Dismiss each one, as for a first-pass audit); '
                + 'anchored findings {id, target ("doc" | "doc:<path>" | "patch:<slug>"), anchor, severity, labels, body, verdict?} are PROPOSED '
                + '(the reviewer Accepts or Dismisses; only accepted ones come back in await_review). A rendered page (HTML or Markdown) takes '
                + 'target "doc:<path>" and anchor {type: "text", exact} | {type: "element", selector} | {type: "document"}.',
            inputSchema: {
                reviewId: reviewArg,
                findings: z.array(z.object({
                    path: z.string().optional().describe('File path relative to the repository'),
                    line: z.number().int().min(1).optional(),
                    endLine: z.number().int().min(1).optional(),
                    body: z.string().describe('Markdown'),
                    severity: z.enum(['low', 'medium', 'high']).optional(),
                    id: z.string().optional(),
                    target: z.string().optional(),
                    anchor: z.record(z.string(), z.any()).optional(),
                    labels: z.array(z.string()).optional(),
                    verdict: z.string().optional(),
                    status: z.enum(['open', 'proposed']).optional().describe('Code findings: "proposed" to have the reviewer triage them (default "open")'),
                    proposal: z.object({ before: z.string(), after: z.string(), path: z.string().optional(), startLine: z.number().optional() }).passthrough().optional()
                }).passthrough())
            }
        }, async ({ reviewId, findings }) => {
            const review = await resolveReview(reviewId);
            if (!review) {
                return fail('No review. Call open_review first.');
            }
            const agent = this.agentOf(session);
            const dir = review.bundle?.dir ?? FileUri.fsPath(review.workspaceRoot);
            const ids: string[] = [];
            for (const finding of findings) {
                if (finding.anchor) {
                    await this.bundles.addFinding(review.id, dir, this.bundles.documentOf(dir), {
                        id: finding.id ?? randomUUID(), target: finding.target, anchor: finding.anchor, body: finding.body,
                        severity: finding.severity as Severity | undefined, labels: finding.labels, verdict: finding.verdict, proposal: finding.proposal
                    }, agent);
                } else if (finding.path) {
                    const location = finding.line
                        ? await this.locationFor(review, finding.path, finding.line, finding.endLine)
                        : await this.pathLocation(review, finding.path);
                    await this.store.createThread(review.id, location, finding.body, agent, {
                        intent: 'comment', severity: finding.severity as Severity | undefined, labels: finding.labels,
                        origin: 'finding', status: finding.status
                    });
                } else {
                    return fail('Each finding needs either `anchor` (with `target`) or `path`.');
                }
                const latest = await this.store.get(review.id);
                ids.push(`t${latest!.nextThreadNumber - 1}`);
            }
            return json({ created: ids.length, ids, hint: 'Findings with an anchor are proposed: only the ones the reviewer accepts come back in await_review.' });
        });

        server.registerTool('ask_reviewer', {
            description: 'Ask the reviewer to decide between options (shown as buttons in the thread). Blocks until answered or timeout.',
            inputSchema: {
                reviewId: reviewArg,
                question: z.string(),
                options: z.array(z.string()).min(1).max(6),
                threadId: z.string().optional().describe('Ask inside this thread; otherwise a repository-level thread is created'),
                timeoutSec: z.number().int().min(1).max(3600).optional()
            }
        }, async ({ reviewId, question, options, threadId, timeoutSec }) => {
            const review = await resolveReview(reviewId);
            if (!review) {
                return fail('No review. Call open_review first.');
            }
            const agent = this.agentOf(session);
            const id = threadId ?? (await this.store.createThread(review.id, { kind: 'repository' }, question, agent)).id;
            const request = { id: randomUUID(), title: question, options: options.map((name, i) => ({ id: String(i), name, kind: 'allow_once' })) };
            const messageId = await this.store.startMessage(review.id, id, agent, { body: threadId ? question : '', permission: request, status: 'done' });
            await this.store.setAgentState(review.id, id, 'waiting_for_human');
            const choice = await this.decisions.wait(request.id, (timeoutSec ?? 900) * 1000);
            await this.store.updateMessage(review.id, id, messageId, { permission: { ...request, outcome: choice ?? 'cancelled' } });
            await this.store.setAgentState(review.id, id, 'idle');
            return json(choice === undefined ? { status: 'pending' } : { status: 'answered', choice: options[Number(choice)] });
        });

        server.registerTool('repo_map', {
            description: 'Outline of the repository: every source file with the classes, functions and methods it defines (Tree-sitter). '
                + 'Built on demand, cached, and refreshed for changed files. Use it to find where something lives before reading or '
                + 'searching files; narrow it with `path` (a folder or file) or `query` (a name or word).',
            inputSchema: {
                root: rootArg,
                path: z.string().optional().describe('Only files under this repository-relative folder (or this file)'),
                query: z.string().optional().describe('Only files whose path or symbol names contain this (case-insensitive)'),
                refresh: z.boolean().optional().describe('Re-check every file now instead of reusing a map from the last few seconds'),
                overview: z.boolean().optional().describe('Instead of the file list: the repository overview (where to start, the areas of '
                    + 'the code and how they connect; from graphify-out/graph.json when the repository has a Graphify graph)')
            }
        }, async ({ root, path: under, query, refresh, overview }) => {
            const review = await resolveReview(undefined, root ? path.resolve(root) : undefined);
            const workspace = root ? path.resolve(root) : review && !review.bundle ? FileUri.fsPath(review.workspaceRoot) : defaultRoot;
            if (!workspace) {
                return fail('No repository: pass `root` (absolute path).');
            }
            if (refresh) {
                await this.index.get(workspace, true);
            }
            if (overview) {
                return { content: [{ type: 'text', text: await this.index.overview(workspace, review && !review.bundle ? review.viewed : undefined) }] };
            }
            return { content: [{ type: 'text', text: await this.index.render(workspace, { path: under, query }) }] };
        });

        server.registerTool('get_review', {
            description: 'Non-blocking snapshot: the latest verdict, open comments (the await_review batch), all threads with location and messages, '
                + 'and `coverage` (repository reviews): source files the reviewer marked as viewed, overall and per area.',
            inputSchema: { reviewId: reviewArg, root: rootArg }
        }, async ({ reviewId, root }) => {
            const review = await resolveReview(reviewId, root ? path.resolve(root) : undefined);
            if (!review) {
                return fail('No review found.');
            }
            // Which parts of the repository the reviewer has looked at, so the agent can point at what's left.
            const coverage = review.bundle ? undefined : await this.index.coverage(FileUri.fsPath(review.workspaceRoot), review.viewed).catch(() => undefined);
            return json({ ...this.batch(review), coverage, threads: review.threads.map(t => this.describe(review, t)) });
        });

        return server;
    }

    /**
     * Claude Code channels: pushes the reviewer's questions and submissions into the agent's session as they happen,
     * so it answers without polling. Only for Claude Code clients; a session not started with channels ignores the
     * notifications. Pushed questions are not marked delivered, so await_comment still returns them in that case.
     */
    protected startChannel(session: Session, opened: Review, agent: Participant): void {
        session.channel?.dispose();
        if (!/claude/i.test(session.server.server.getClientVersion()?.name ?? '')) {
            return;
        }
        const pushed = new Set<string>();
        let submits = opened.verdict?.count ?? 0;
        const notify = (content: string, meta: Record<string, string>) =>
            session.server.server.notification({ method: 'notifications/claude/channel', params: { content, meta } });
        const push = async () => {
            const review = await this.store.get(opened.id);
            if (!review) {
                return;
            }
            for (const thread of this.waitingThreads(review, agent, session)) {
                const last = thread.messages[thread.messages.length - 1];
                if (!pushed.has(last.id)) {
                    pushed.add(last.id);
                    await notify(this.channelText(review, thread), { review_id: review.id, thread_id: threadRef(thread), event: 'comment' });
                }
            }
            const verdict = review.verdict;
            if (verdict && verdict.count > submits) {
                submits = verdict.count;
                await notify(`The reviewer submitted round ${verdict.count}: ${verdict.decision}${verdict.summary ? ` — "${verdict.summary}"` : ''}. `
                    + 'Call await_review (it returns immediately) for the open comments and accepted suggestions.', { review_id: review.id, event: 'submitted' });
            }
        };
        const run = () => push().catch(error => console.error('[co-review] channel push failed', error));
        const listener = this.store.onDidChange(change => change.kind === 'changed' && change.review.id === opened.id && run());
        session.channel = listener;
        run();
    }

    /** A thread as a channel message: where it is, the code, and the conversation. */
    protected channelText(review: Review, thread: ReviewThread): string {
        const t = this.describe(review, thread) as { location: { path?: string; startLine?: number; endLine?: number; symbol?: string }; code?: string;
            messages: { author: string; body: string }[] };
        const l = t.location;
        const where = l.path ? `${l.path}${l.startLine ? `:${l.startLine}${l.endLine && l.endLine !== l.startLine ? `-${l.endLine}` : ''}` : ''}` : 'the repository';
        return [
            `#${thread.number} on ${where}${l.symbol ? ` (${l.symbol})` : ''}${thread.intent === 'question' ? ', a question for you' : ''}`,
            t.code ? t.code.split('\n').map(line => `> ${line}`).join('\n') : '',
            ...t.messages.map(m => `${m.author}: ${m.body}`),
            `Answer with reply({ threadId: "${threadRef(thread)}" }). ${ANSWER_STYLE_SHORT}`
        ].filter(Boolean).join('\n\n');
    }

    /** Open threads whose latest message is from the reviewer and addressed to this agent. */
    protected waitingThreads(review: Review, agent: Participant, session: Session): ReviewThread[] {
        return review.threads.filter(thread => {
            const last = thread.messages[thread.messages.length - 1];
            if (thread.status !== 'open' || !last || last.author.kind !== 'human' || session.delivered.has(last.id)) {
                return false;
            }
            // Questions go to whoever holds the review's agent seat; follow-ups to the agents in the thread.
            const seated = review.agent?.transport === 'mcp' && review.agent.id === agent.id;
            // A review directory is the agent's own work: every reviewer comment is for it.
            return ((thread.intent === 'question' || !!review.bundle) && seated) || thread.messages.some(m => m.author.id === agent.id);
        });
    }

    protected root(review: Review): string {
        return review.bundle?.dir ?? FileUri.fsPath(review.workspaceRoot);
    }

    /** The document's design-format check, so the agent learns about departures in the loop. */
    protected formatOf(docFile: string | undefined): { format?: DocumentFormat & { next?: string } } {
        if (!docFile) {
            return {};
        }
        try {
            const format = documentFormat(require('fs').readFileSync(docFile, 'utf8'), path.basename(docFile));
            if (!format.declared && format.mode === 'design') {
                format.warnings.push('Looks like a design document: add frontmatter `co-review: design` to declare (and check) the structure.');
            }
            return format.warnings.length
                ? { format: { ...format, next: 'Fix each warning in the document, then call open_review again before giving the reviewer the URL.' } }
                : { format };
        } catch {
            return {};
        }
    }

    /** The await_review / get_review batch. */
    protected batch(review: Review): Record<string, unknown> {
        const open = review.threads.filter(t => t.status === 'open');
        const docFile = review.bundle ? this.bundles.documentOf(review.bundle.dir) : undefined;
        return {
            status: review.verdict ? 'submitted' : 'no-submit',
            reviewId: review.id,
            decision: review.verdict?.decision,
            summary: review.verdict?.summary,
            submittedAt: review.verdict && new Date(review.verdict.submittedAt).toISOString(),
            round: review.verdict?.count ?? 0,
            doc: { slug: docFile ? path.basename(docFile) : '', version: review.bundle?.docVersion ?? 1, ...this.formatOf(docFile) },
            comments: open.map(t => commentOf(t, this.root(review))),
            acceptedSuggestions: acceptedSuggestions(review),
            openCount: open.length,
            proposedCount: review.threads.filter(t => t.status === 'proposed').length
        };
    }

    /** The await_comment batch, plus the threads to answer with full context. */
    protected commentBatch(review: Review, waiting: ReviewThread[]): Record<string, unknown> {
        const open = review.threads.filter(t => t.status === 'open');
        const needs = (t: ReviewThread) => t.messages[t.messages.length - 1]?.author.kind === 'human';
        return {
            status: 'comment',
            reviewId: review.id,
            comments: open.map(t => commentOf(t, this.root(review), { lastRole: t.messages[t.messages.length - 1]?.author.kind, needsReply: needs(t) })),
            openCount: open.length,
            needsReply: open.filter(needs).length,
            threads: waiting.map(t => this.describe(review, t)),
            hint: 'reply(threadId, body) to answer (edit the reviewed files on disk if needed — the view live-reloads), then call await_comment again.',
            howToAnswer: ANSWER_STYLE_SHORT
        };
    }

    protected describe(review: Review, thread: ReviewThread): object {
        const root = this.root(review);
        const location = thread.location;
        return {
            threadId: thread.id,
            id: threadRef(thread),
            number: thread.number,
            status: thread.status,
            intent: thread.intent,
            severity: thread.severity,
            labels: thread.labels,
            anchor: location.docAnchor ?? location.patchAnchor,
            location: {
                kind: location.kind,
                path: location.patchAnchor?.path ?? (location.uri ? path.relative(root, FileUri.fsPath(location.uri)) : undefined),
                startLine: location.range ? location.range.start.line + 1 : location.patchAnchor?.line ?? location.patchAnchor?.startLine,
                endLine: location.range ? location.range.end.line + 1 : location.patchAnchor?.line ?? location.patchAnchor?.endLine,
                symbol: location.symbol
            },
            code: location.kind !== 'repository' ? location.anchor?.text : undefined,
            messages: thread.messages.filter(m => m.body.trim()).map(m => ({ author: m.author.name, role: m.author.kind, body: m.body }))
        };
    }

    /** A file or folder of the repository (`.` or `/`: the repository itself). */
    protected async pathLocation(review: Review, relative: string): Promise<CodeLocation> {
        const root = FileUri.fsPath(review.workspaceRoot);
        const file = path.resolve(root, relative);
        if (file === root) {
            return { kind: 'repository' };
        }
        const stat = await fs.stat(file).catch(() => undefined);
        return { kind: stat?.isDirectory() ? 'directory' : 'file', uri: FileUri.create(file).toString() };
    }

    /** Builds a semantic location (range, anchor, Tree-sitter tokens, symbol) from path + lines. */
    protected async locationFor(review: Review, relative: string, line: number, endLine?: number): Promise<CodeLocation> {
        const root = FileUri.fsPath(review.workspaceRoot);
        const file = path.resolve(root, relative);
        const uri = FileUri.create(file).toString();
        const text = await fs.readFile(file, 'utf8').catch(() => undefined);
        if (text === undefined) {
            return { kind: 'file', uri };
        }
        const lines = text.split('\n');
        const start = Math.min(Math.max(line, 1), lines.length) - 1;
        const end = Math.min(Math.max(endLine ?? line, line), lines.length) - 1;
        const range = { start: { line: start, character: 0 }, end: { line: end, character: lines[end].length } };
        const languageId = LANGUAGE_BY_EXTENSION[path.extname(file)];
        const syntax = languageId ? await this.syntax.captureAnchor(languageId, text, range).catch(() => undefined) : undefined;
        const symbols = languageId ? await this.syntax.getSymbols(languageId, text).catch(() => []) : [];
        return {
            kind: start === end ? 'line' : 'range',
            uri,
            range,
            symbol: this.enclosingSymbol(symbols, start),
            anchor: { text: lines.slice(start, end + 1).join('\n'), before: lines[start - 1], after: lines[end + 1], syntax }
        };
    }

    protected enclosingSymbol(symbols: SyntaxSymbol[], line: number, parents: string[] = []): string | undefined {
        for (const symbol of symbols) {
            if (symbol.range.start.line <= line && line <= symbol.range.end.line) {
                const path = [...parents, symbol.name];
                return this.enclosingSymbol(symbol.children, line, path) ?? path.join('.');
            }
        }
        return undefined;
    }

    protected nextChange(reviewId: string, timeoutMs: number, signal: AbortSignal): Promise<void> {
        return new Promise(resolve => {
            const done = () => {
                clearTimeout(timer);
                listener.dispose();
                signal.removeEventListener('abort', done);
                resolve();
            };
            const timer = setTimeout(done, Math.max(0, timeoutMs));
            const listener = this.store.onDidChange(change => change.kind === 'changed' && change.review.id === reviewId && done());
            signal.addEventListener('abort', done);
        });
    }
}
