#!/usr/bin/env node
/**
 * Records the Theia app's MCP server (extensions/review/lib/node/co-reviewer-mcp.js) driving the four workflows
 * (audit, change, PR, design) with a scripted reviewer that acts through the review store. Writes one JSONL file per
 * workflow to tests/conformance/, which tests/conformance.rs replays against review_mcp.
 *
 * Each line is a step: `initialize` (the server's instructions and capabilities), `tools` (tools/list), `call` (a tool
 * call with its result; `during` are reviewer actions taken while the call waits), `reviewer` (an action through the
 * store) or `notifications` (Claude Code channel notifications received so far, as a sorted list). Paths are written
 * with placeholders ({repo}, {dir}, {tmp}); the replay substitutes its own.
 *
 * Usage: NODE_PATH=<co-review checkout>/node_modules node tests/gen/record.mjs
 */

import { execFileSync } from 'child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'fs';
import { createRequire } from 'module';
import { tmpdir } from 'os';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const here = dirname(fileURLToPath(import.meta.url));
const crateRoot = join(here, '..', '..');
const repoRoot = join(crateRoot, '..', '..');
const lib = join(repoRoot, 'extensions/review/lib/node');
const require = createRequire(join(lib, 'co-reviewer-mcp.js'));

// The desktop app's behaviour: open_review returns a note and opens nothing (no browser, no app launch).
Object.defineProperty(process.versions, 'electron', { value: '0.0.0-record' });
delete process.env.CO_REVIEW_APP_LAUNCH;

const tmp = realpathSync(mkdtempSync(join(tmpdir(), 'co-review-record-')));
process.env.CO_REVIEW_HOME = join(tmp, 'home');

require('reflect-metadata');
const { Container } = require('@theia/core/shared/inversify');
const express = require('@theia/core/shared/express');
const { Client } = require('@modelcontextprotocol/sdk/client/index.js');
const { StreamableHTTPClientTransport } = require('@modelcontextprotocol/sdk/client/streamableHttp.js');
const { FileUri } = require('@theia/core/lib/common/file-uri');
const load = name => require(join(lib, name));
const { ReviewStore } = load('review-store.js');
const { HumanDecisions, AgentPresenceTracker, ReviewWindows } = load('agent-coordination.js');
const { CoReviewerMcp } = load('co-reviewer-mcp.js');
const { SyntaxServiceImpl } = load('syntax-service-impl.js');
const { BundleService } = load('bundle-service.js');
const { GitHubReviews } = load('github.js');
const { RepoIndex } = load('repo-index.js');
const { CodeFolders } = load('code-folders.js');
const { AnchorTracker } = load('anchor-tracker.js');

const container = new Container();
for (const service of [ReviewStore, HumanDecisions, AgentPresenceTracker, ReviewWindows, CoReviewerMcp, SyntaxServiceImpl,
    BundleService, GitHubReviews, RepoIndex, CodeFolders, AnchorTracker]) {
    container.bind(service).toSelf().inSingletonScope();
}
const store = container.get(ReviewStore);
const decisions = container.get(HumanDecisions);
const presence = container.get(AgentPresenceTracker);
const mcp = container.get(CoReviewerMcp);

const app = express();
mcp.configure(app);
const server = await new Promise(resolve => {
    const s = app.listen(0, '127.0.0.1', () => resolve(s));
});
await mcp.onStart(server);
const port = server.address().port;

const reviewer = { id: 'human:reviewer@example.com', kind: 'human', name: 'Reviewer' };

function write(file, text) {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
}

/** A small repository with a reviewer identity (so choices are recorded as the same person), as fixture operations. */
function repository() {
    return [
        { op: 'write', path: '{repo}/README.md', text: '# Shop\n\nOrders and payments.\n' },
        { op: 'write', path: '{repo}/src/main.ts', text: 'import { createOrder } from "./orders/service";\n\nexport function main(): void {\n    createOrder("book", 2);\n}\n' },
        { op: 'write', path: '{repo}/src/orders/service.ts', text: [
            'export interface Order {',
            '    item: string;',
            '    quantity: number;',
            '}',
            '',
            'export function createOrder(item: string, quantity: number): Order {',
            '    if (quantity <= 0) {',
            '        throw new Error("quantity");',
            '    }',
            '    return { item, quantity };',
            '}',
            ''
        ].join('\n') },
        { op: 'write', path: '{repo}/src/payments/charge.py', text: 'def charge(amount):\n    return amount * 100\n\n\nclass Gateway:\n    def send(self, amount):\n        return charge(amount)\n' },
        { op: 'git', args: ['init', '-q', '-b', 'main', '{repo}'] },
        { op: 'git', args: ['-C', '{repo}', 'config', 'user.name', 'Reviewer'] },
        { op: 'git', args: ['-C', '{repo}', 'config', 'user.email', 'reviewer@example.com'] },
        { op: 'git', args: ['-C', '{repo}', 'add', '-A'] },
        { op: 'git', args: ['-C', '{repo}', 'commit', '-q', '-m', 'Initial'] }
    ];
}

/** Runs fixture operations (paths and arguments with placeholders). */
function prepare(operations) {
    for (const operation of operations) {
        if (operation.op === 'write') {
            write(substitute(operation.path), operation.text);
        } else if (operation.op === 'git') {
            execFileSync('git', substitute(operation.args), { cwd: tmp, encoding: 'utf8' });
        }
    }
}

const placeholders = [];
/** The review the last open_review returned, for `{reviewId}` in arguments. */
const current = { reviewId: undefined };
function substitute(value) {
    if (typeof value === 'string') {
        return [...placeholders, ['reviewId', current.reviewId ?? '']].reduce((text, [name, path]) => text.split(`{${name}}`).join(path), value);
    }
    if (Array.isArray(value)) {
        return value.map(substitute);
    }
    if (value && typeof value === 'object') {
        return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, substitute(v)]));
    }
    return value;
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function until(condition, what) {
    for (let i = 0; i < 400; i++) {
        if (await condition()) {
            return;
        }
        await sleep(25);
    }
    throw new Error(`timed out waiting for ${what}`);
}

class Session {
    constructor(clientName, root) {
        this.notifications = [];
        this.client = new Client({ name: clientName, version: '1.0.0' });
        this.client.fallbackNotificationHandler = async notification => { this.notifications.push(notification); };
        this.url = new URL(`http://127.0.0.1:${port}/mcp${root ? `?root=${encodeURIComponent(root)}` : ''}`);
    }
    async connect() {
        await this.client.connect(new StreamableHTTPClientTransport(this.url));
    }
}

/** Runs one workflow and writes its steps with their results. */
async function record({ name, client: clientName, placeholders: names, fixture, steps }) {
    if (process.env.ONLY && process.env.ONLY !== name) {
        return;
    }
    placeholders.splice(0, placeholders.length, ['tmp', tmp]);
    for (const [key, value] of Object.entries(names)) {
        placeholders.push([key, substitute(value)]);
    }
    prepare(fixture);
    const lines = [{ kind: 'fixture', placeholders: names, operations: fixture }];
    const root = substitute('{repo}');
    const session = new Session(clientName, root);
    await session.connect();
    lines.push({ kind: 'session', client: clientName, root: '{repo}' });
    lines.push({ kind: 'initialize', instructions: session.client.getInstructions(), capabilities: session.client.getServerCapabilities() });
    lines.push({ kind: 'tools', tools: (await session.client.listTools()).tools });
    const state = { reviewId: undefined };
    for (const step of steps) {
        if (step.kind === 'call') {
            const call = session.client.callTool({ name: step.tool, arguments: substitute(step.arguments) }, undefined, { timeout: 600_000 });
            for (const action of step.during ?? []) {
                await waitFor(action, state);
                await act(action, state);
            }
            const result = await call;
            const text = result.content?.[0]?.text;
            try {
                const parsed = JSON.parse(text);
                if (step.tool === 'open_review' && parsed.reviewId) {
                    state.reviewId = parsed.reviewId;
                    current.reviewId = parsed.reviewId;
                }
            } catch {
                /* plain text */
            }
            lines.push({ ...step, result: { content: result.content, isError: result.isError ?? false } });
            if (process.env.VERBOSE) {
                console.log('>>', step.tool, JSON.stringify(step.arguments), '\n', text?.slice(0, 600));
            }
        } else if (step.kind === 'reviewer') {
            await act(step, state);
            lines.push(step);
        } else if (step.kind === 'notifications') {
            await sleep(500);
            lines.push({ kind: 'notifications', notifications: session.notifications.map(n => ({ method: n.method, params: n.params }))
                .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))) });
            session.notifications.length = 0;
        }
    }
    await session.client.close();
    const out = join(crateRoot, 'tests/conformance', `${name}.jsonl`);
    mkdirSync(dirname(out), { recursive: true });
    // Paths under the scratch directory are written as {tmp}, which the replay substitutes with its own.
    const unscratched = line => JSON.stringify(line).split(tmp).join('{tmp}').split(tmp.replace(/^\/private/, '')).join('{tmp}');
    writeFileSync(out, lines.map(unscratched).join('\n') + '\n');
    console.log(`${name}: ${lines.length} steps -> ${out}`);
}

function threadOf(review, ref) {
    const number = Number(ref.slice(1));
    const thread = review.threads.find(t => t.number === number);
    if (!thread) {
        throw new Error(`no thread ${ref}`);
    }
    return thread;
}

function pendingPermission(review) {
    for (const thread of review.threads) {
        for (const message of thread.messages) {
            if (message.permission && !message.permission.outcome) {
                return message.permission;
            }
        }
    }
    return undefined;
}

/** Waits until the call is blocked where the action makes sense: a question is shown, or the agent is listening. */
async function waitFor(action, state) {
    if (action.action === 'choose') {
        await until(async () => pendingPermission(await store.get(state.reviewId)), 'a question to the reviewer');
    } else {
        await until(() => presence.get(state.reviewId)?.listening, 'the agent to listen');
    }
}

async function act(step, state) {
    const reviewId = state.reviewId;
    const review = await store.get(reviewId);
    switch (step.action) {
        case 'ask': {
            const thread = threadOf(review, step.thread);
            await store.addMessage(reviewId, thread.id, step.body, reviewer);
            await store.setThreadIntent(reviewId, thread.id, 'question');
            return;
        }
        case 'comment':
            await store.addMessage(reviewId, threadOf(review, step.thread).id, step.body, reviewer);
            return;
        case 'newThread':
            await store.createThread(reviewId, substitute(step.location), step.body, reviewer, step.options ?? {});
            return;
        case 'choose': {
            const permission = pendingPermission(review);
            decisions.resolve(permission.id, step.option);
            // The choice is written to the thread after the decision resolves.
            await sleep(200);
            return;
        }
        case 'submit':
            await store.submit(reviewId, step.decision, step.summary, reviewer);
            return;
        case 'accept':
            await store.decideProposal(reviewId, threadOf(review, step.thread).id, true, step.note);
            return;
        case 'viewed':
            await store.setViewed(reviewId, step.paths, true);
            return;
        case 'close': {
            const root = review.bundle?.code ?? review.bundle?.dir ?? FileUri.fsPath(review.workspaceRoot);
            mcp.markClosed(FileUri.create(root).toString());
            await Promise.all(mcp.closing);
            return;
        }
        default:
            throw new Error(`unknown reviewer action ${step.action}`);
    }
}


// ---------------------------------------------------------------- audit: the whole repository
await record({
    name: 'audit', client: 'claude-code', placeholders: { repo: '{tmp}/audit' }, fixture: repository(), steps: [
        { kind: 'call', tool: 'open_review', arguments: { title: 'Audit of the shop', open: false } },
        { kind: 'call', tool: 'repo_map', arguments: {} },
        { kind: 'call', tool: 'repo_map', arguments: { path: 'src/orders' } },
        { kind: 'call', tool: 'repo_map', arguments: { query: 'charge' } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [
            { path: 'src/orders/service.ts', line: 7, endLine: 9, body: 'A zero quantity throws a bare Error.', severity: 'medium', labels: ['orders'], status: 'proposed' },
            { path: 'src/payments', body: 'Payments have no tests.', severity: 'high', labels: ['payments'], status: 'proposed' },
            { path: '.', body: 'No CI configuration.', labels: ['repository'] },
            { path: 'src/main.ts', line: 4, body: 'Hard-coded order.', severity: 'low', labels: ['entry'] }
        ] } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [{ path: '../outside.ts', line: 1, body: 'x' }] } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [{ body: 'Neither path nor anchor.' }] } },
        { kind: 'call', tool: 'open_review', arguments: { overview: '# The shop\n\nOrders and payments.', open: false } },
        { kind: 'call', tool: 'open_review', arguments: {} },
        { kind: 'call', tool: 'get_review', arguments: {} },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 1 } },
        { kind: 'reviewer', action: 'ask', thread: 't1', body: 'Why does it throw instead of returning an error?' },
        { kind: 'notifications' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 30 } },
        { kind: 'call', tool: 'reply', arguments: { threadId: 't1', body: 'It treats a bad quantity as a programming error. src/orders/service.ts:8' } },
        { kind: 'call', tool: 'reply', arguments: { threadId: 't99', body: 'nobody' } },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 30 }, during: [
            { action: 'newThread', location: { kind: 'line', uri: 'file://{repo}/src/payments/charge.py', range: { start: { line: 1, character: 0 }, end: { line: 1, character: 24 } }, anchor: { text: '    return amount * 100' } },
                body: 'Is this cents?', options: { intent: 'question' } }
        ] },
        { kind: 'call', tool: 'reply', arguments: { threadId: 't5', body: 'Yes, it converts to cents.', resolve: true } },
        { kind: 'call', tool: 'ask_reviewer', arguments: { question: 'Split payments into its own package?', options: ['Yes', 'No', 'Later'], timeoutSec: 60 }, during: [
            { action: 'choose', option: '2' }
        ] },
        { kind: 'call', tool: 'ask_reviewer', arguments: { question: 'Which?', options: ['A'], threadId: 't42' } },
        { kind: 'reviewer', action: 'viewed', paths: ['src/main.ts', 'README.md'] },
        { kind: 'call', tool: 'get_review', arguments: {} },
        { kind: 'reviewer', action: 'submit', decision: 'request-changes', summary: 'Fix the payments tests.' },
        { kind: 'notifications' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 1 } },
        { kind: 'reviewer', action: 'close' },
        { kind: 'notifications' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 1 } },
        { kind: 'call', tool: 'open_review', arguments: { reviewId: '00000000-0000-0000-0000-000000000000' } },
        { kind: 'call', tool: 'post_review_to_github', arguments: {} },
        { kind: 'call', tool: 'repo_map', arguments: { overview: true } }
    ]
});

// ---------------------------------------------------------------- change: your own uncommitted change
await record({
    name: 'change', client: 'codex', placeholders: { repo: '{tmp}/change' }, fixture: [
        ...repository(),
        { op: 'write', path: '{repo}/src/orders/service.ts', text: [
            'export interface Order {',
            '    item: string;',
            '    quantity: number;',
            '}',
            '',
            'export function createOrder(item: string, quantity: number): Order {',
            '    if (!Number.isInteger(quantity) || quantity <= 0) {',
            '        throw new RangeError(`quantity must be a positive integer, got ${quantity}`);',
            '    }',
            '    return { item, quantity };',
            '}',
            ''
        ].join('\n') },
        { op: 'write', path: '{repo}/src/orders/limits.ts', text: 'export const MAX_QUANTITY = 100;\n' }
    ], steps: [
        { kind: 'call', tool: 'open_review', arguments: { diff: 'HEAD;rm', root: '{repo}', open: false } },
        { kind: 'call', tool: 'open_review', arguments: { diff: 'main...main', root: '{repo}', open: false } },
        { kind: 'call', tool: 'open_review', arguments: { diff: 'HEAD', root: '{repo}', title: 'Validate quantities', open: false } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [
            { id: 'f1', target: 'patch:change', anchor: { type: 'code-line', path: 'src/orders/service.ts', line: 8, side: 'new', source: 'throw new RangeError' },
                body: 'RangeError is right, but callers catch Error.', severity: 'medium', labels: ['orders'], verdict: 'Consider a custom error.' },
            { id: 'f2', target: 'patch:change', anchor: { type: 'code-file', path: 'src/orders/limits.ts' }, body: 'MAX_QUANTITY is unused.', severity: 'low',
                proposal: { before: 'export const MAX_QUANTITY = 100;', after: '', path: 'src/orders/limits.ts', startLine: 1 } },
            { id: 'f3', target: 'doc:notes.txt', anchor: { type: 'text', exact: 'x' }, body: 'not a page' }
        ] } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [
            { id: 'f4', target: 'patch:change', anchor: { type: 'patch' }, body: 'The change has no tests.', severity: 'high' }
        ] } },
        { kind: 'call', tool: 'open_review', arguments: { reviewId: '{reviewId}', overview: 'Validates order quantities.', open: false } },
        { kind: 'call', tool: 'open_review', arguments: { reviewId: '{reviewId}' } },
        { kind: 'reviewer', action: 'comment', thread: 't1', body: 'Which callers?' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'reply', arguments: { threadId: 't1', body: 'Only main.ts, which does not catch at all.' } },
        { kind: 'reviewer', action: 'accept', thread: 't2', note: 'Suggestion accepted. Apply it as a new commit on the branch; the reviewed patch stays as it is.' },
        { kind: 'reviewer', action: 'submit', decision: 'approve', summary: 'Good.' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'get_review', arguments: {} },
        // Without reviewId or title, open_review falls back to the repository's own review: a new one here.
        { kind: 'call', tool: 'open_review', arguments: { overview: 'Validates order quantities.', open: false } }
    ]
});

// ---------------------------------------------------------------- pr: someone else's pull request
await record({
    name: 'pr', client: 'codex', placeholders: { repo: '{tmp}/pr', dir: '{tmp}/pr-review' }, fixture: [
        ...repository(),
        { op: 'git', args: ['-C', '{repo}', 'checkout', '-q', '-b', 'feature/discounts'] },
        { op: 'write', path: '{repo}/src/orders/discount.ts', text: 'export function discount(total: number): number {\n    return total > 100 ? total * 0.9 : total;\n}\n' },
        { op: 'git', args: ['-C', '{repo}', 'add', '-A'] },
        { op: 'git', args: ['-C', '{repo}', 'commit', '-q', '-m', 'Discounts'] },
        { op: 'git', args: ['-C', '{repo}', 'checkout', '-q', 'main'] },
        { op: 'write', path: '{dir}/PR.md', text: 'title: Discounts over 100\npr: 42\nrepo: acme/shop\nbranch: feature/discounts\nbase: main\n\nTen percent off large orders.\n' }
    ], steps: [
        { kind: 'call', tool: 'open_review', arguments: { dir: '{dir}', root: '{repo}', diff: 'main...feature/discounts', patchName: 'pr-42', open: false } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [
            { id: 'p1', target: 'patch:pr-42', anchor: { type: 'code-line', path: 'src/orders/discount.ts', line: 2, side: 'new', source: 'return total > 100' },
                body: 'Magic numbers: name the threshold and the rate.', severity: 'medium', labels: ['orders'],
                proposal: { before: '    return total > 100 ? total * 0.9 : total;', after: '    return total > THRESHOLD ? total * RATE : total;' } },
            { id: 'p2', target: 'patch:pr-42', anchor: { type: 'code-range', path: 'src/orders/discount.ts', startLine: 1, endLine: 3, side: 'new' }, body: 'No tests for the boundary.', severity: 'high' }
        ] } },
        { kind: 'call', tool: 'open_review', arguments: { reviewId: '{reviewId}' } },
        { kind: 'reviewer', action: 'accept', thread: 't1', note: 'Suggestion accepted. Apply it as a new commit on feature/discounts; the reviewed patch stays as it is.' },
        { kind: 'reviewer', action: 'comment', thread: 't2', body: 'Is exactly 100 discounted?' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'reply', arguments: { threadId: 't2', body: 'No: only totals above 100.' } },
        { kind: 'reviewer', action: 'submit', decision: 'request-changes', summary: 'Name the constants and add a boundary test.' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'post_review_to_github', arguments: { summary: 'Please name the constants.', comments: [{ threadId: 't2', body: 'Add a test for exactly 100.' }] }, during: [
            { action: 'choose', option: 'skip' }
        ] },
        { kind: 'call', tool: 'post_review_to_github', arguments: { comments: [{ threadId: 't77', body: 'x' }] } },
        { kind: 'call', tool: 'get_review', arguments: {} }
    ]
});

// ---------------------------------------------------------------- design: a design document
await record({
    name: 'design', client: 'codex', placeholders: { repo: '{tmp}/design', dir: '{tmp}/design/design/rate-limit' }, fixture: [
        ...repository(),
        { op: 'write', path: '{dir}/index.markdown', text: [
        '---',
        'co-review: design',
        '---',
        '# Limit order requests per customer',
        '',
        '## Context',
        '',
        'Customers can place unlimited orders; a script placed 10,000 last week.',
        '',
        '## Design',
        '',
        '- Each customer gets a token bucket',
        '  - Refilled at 10 orders per minute, see src/orders/service.ts:6',
        '    - A restart empties the buckets',
        '- Requests over the limit get 429',
        '',
        '## Open Questions',
        '',
        '- Should staff accounts be exempt?',
        ''
    ].join('\n') }
    ], steps: [
        { kind: 'call', tool: 'open_review', arguments: { dir: '{dir}', open: false } },
        { kind: 'call', tool: 'add_findings', arguments: { findings: [
            { id: 'd1', target: 'doc', anchor: { type: 'text', exact: 'A restart empties the buckets' }, body: 'Is losing the buckets acceptable?', severity: 'medium' },
            { id: 'd2', target: 'doc', anchor: { type: 'tree-node', nodeId: 'Requests over the limit get 429' }, body: 'Add Retry-After.', severity: 'low',
                proposal: { before: 'Requests over the limit get 429', after: 'Requests over the limit get 429 with Retry-After' } }
        ] } },
        { kind: 'call', tool: 'open_review', arguments: { reviewId: '{reviewId}' } },
        { kind: 'reviewer', action: 'newThread', location: { kind: 'document', uri: 'file://{dir}/index.markdown', docAnchor: { type: 'text', exact: 'Customers can place unlimited orders' }, anchor: { text: 'Customers can place unlimited orders' } },
            body: 'Where does the 10,000 come from?', options: {} },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'reply', arguments: { threadId: 't3', body: 'From last week\'s order log; I will cite it.' } },
        { kind: 'reviewer', action: 'submit', decision: 'approve', summary: 'Go ahead.' },
        { kind: 'call', tool: 'await_reviewer', arguments: { timeoutSec: 5 } },
        { kind: 'call', tool: 'get_review', arguments: {} }
    ]
});

server.close();
rmSync(tmp, { recursive: true, force: true });
process.exit(0);
