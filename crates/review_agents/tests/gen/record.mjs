#!/usr/bin/env node
/**
 * Records the Theia app's ACP agent service (extensions/review/lib/node/acp-agent-service.js) answering review threads
 * with the scripted agent of tests/support/fake_agent.rs. Writes tests/fixtures/acp_turns.json, which
 * tests/conformance.rs replays against review_agents.
 *
 * The fixture holds the script (`steps`) and, per step, the agent's message in the thread and the prompts the agent
 * received; then what the client sent at initialize, session/new and session/set_config_option, and the settings the
 * agent offered. Paths are written with placeholders ({repo}, {outside}); the replay substitutes its own.
 *
 * Usage: cargo build -p review_agents --bin fake-acp-agent
 *        NODE_PATH=<co-review checkout>/node_modules node crates/review_agents/tests/gen/record.mjs [path/to/fake-acp-agent]
 */

import { execFileSync } from 'child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync, existsSync } from 'fs';
import { createRequire } from 'module';
import { tmpdir } from 'os';
import { basename, dirname, join } from 'path';
import { fileURLToPath } from 'url';

const here = dirname(fileURLToPath(import.meta.url));
const crateRoot = join(here, '..', '..');
const repoRoot = join(crateRoot, '..', '..');
const lib = join(repoRoot, 'extensions/review/lib/node');
const require = createRequire(join(lib, 'acp-agent-service.js'));

const agentBinary = process.argv[2] ?? join(repoRoot, 'target/debug/fake-acp-agent');
if (!existsSync(agentBinary)) {
    throw new Error(`No fake agent at ${agentBinary}: cargo build -p review_agents --bin fake-acp-agent`);
}

const tmp = realpathSync(mkdtempSync(join(tmpdir(), 'co-review-acp-record-')));
process.env.CO_REVIEW_HOME = join(tmp, 'home');

require('reflect-metadata');
const { Container } = require('@theia/core/shared/inversify');
const { FileUri } = require('@theia/core/lib/common/file-uri');
const load = name => require(join(lib, name));
const { ReviewStore } = load('review-store.js');
const { HumanDecisions } = load('agent-coordination.js');
const { RepoIndex } = load('repo-index.js');
const { SyntaxServiceImpl } = load('syntax-service-impl.js');
const { AcpAgentService } = load('acp-agent-service.js');

const container = new Container();
for (const service of [ReviewStore, HumanDecisions, RepoIndex, SyntaxServiceImpl, AcpAgentService]) {
    container.bind(service).toSelf().inSingletonScope();
}
const store = container.get(ReviewStore);
const decisions = container.get(HumanDecisions);
const agents = container.get(AcpAgentService);

const LIB = 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Calculator;\n\nimpl Calculator {\n    pub fn total(&self) -> i32 {\n        add(1, 2)\n    }\n}\n';

const repo = join(tmp, 'repo');
mkdirSync(join(repo, 'src'), { recursive: true });
writeFileSync(join(repo, 'src/lib.rs'), LIB);
writeFileSync(join(repo, 'README.md'), '# Calculator\n');
execFileSync('git', ['init', '-q'], { cwd: repo });
execFileSync('git', ['add', '.'], { cwd: repo });
const outside = join(tmp, 'outside.txt');
writeFileSync(outside, 'one\nsecret\n');
const log = join(tmp, 'agent.jsonl');

/** The script; tests/conformance.rs runs the same steps. */
const steps = [
    { op: 'connect' },
    { op: 'question', name: 'answer', body: 'What does add do?' },
    { op: 'reply', name: 'follow-up', thread: 'answer', body: 'And what are the settings?' },
    { op: 'settings', settings: { model: 'opus', effort: 'high' } },
    { op: 'question', name: 'settings-new', body: 'What are the settings?' },
    { op: 'reply', name: 'settings-open', thread: 'answer', body: 'settings again?' },
    { op: 'question', name: 'permission', body: 'Fix it, permission needed', decide: 'allow' },
    { op: 'reply', name: 'permission-stop', thread: 'permission', body: 'Again, permission needed', decide: null },
    { op: 'question', name: 'slow-stop', body: 'Take it slow', stopAfter: 'Working on it' },
    { op: 'question', name: 'read', body: 'read:src/lib.rs' },
    { op: 'reply', name: 'read-outside', thread: 'read', body: 'read:../outside.txt' },
    { op: 'reply', name: 'write', thread: 'read', body: 'write:notes/../written.txt' },
    { op: 'reply', name: 'write-outside', thread: 'read', body: 'write:{outside}' },
    { op: 'question', name: 'refuse', body: 'Please refuse' },
    { op: 'question', name: 'repository', body: 'How is the repository laid out?', repository: true },
    { op: 'options' }
];

const reviewer = { id: 'human:reviewer@example.com', kind: 'human', name: 'Reviewer' };
const fileLocation = () => ({
    kind: 'range',
    uri: FileUri.create(join(repo, 'src/lib.rs')).toString(),
    range: { start: { line: 0, character: 0 }, end: { line: 2, character: 1 } },
    symbol: 'add',
    anchor: { text: 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}' }
});

const placeholders = text => text.split(outside).join('{outside}').split(repo).join('{repo}');
const scrub = value => JSON.parse(placeholders(JSON.stringify(value)));
const parse = line => {
    try {
        return [JSON.parse(line)];
    } catch {
        return []; // a line still being written
    }
};
const received = () => existsSync(log) ? readFileSync(log, 'utf8').split('\n').filter(Boolean).flatMap(parse) : [];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function until(read, test, what) {
    const deadline = Date.now() + 20000;
    for (;;) {
        const value = await read();
        if (test(value)) {
            return value;
        }
        if (Date.now() > deadline) {
            throw new Error(`timed out waiting for ${what}: ${JSON.stringify(value, undefined, 1)}`);
        }
        await sleep(20);
    }
}

const review = await store.create({ workspaceRoot: FileUri.create(repo).toString(), title: 'Calculator', scope: { kind: 'repository' }, author: reviewer });
const agentConfig = { id: 'fake', name: 'Fake Agent', transport: 'acp', command: agentBinary, args: [log] };
const threads = new Map();
const thread = async id => (await store.get(review.id)).threads.find(t => t.id === id);
const agentMessages = t => t.messages.filter(m => m.author.kind === 'agent');
const results = [];

for (const step of steps) {
    if (step.op === 'connect') {
        await store.setAgent(review.id, agentConfig);
        continue;
    }
    if (step.op === 'settings') {
        const current = (await store.get(review.id)).agent;
        await store.setAgent(review.id, { ...current, settings: step.settings });
        continue;
    }
    if (step.op === 'options') {
        results.push({ name: 'options', settings: await agents.getSettings(review.id) });
        continue;
    }
    const before = received().length;
    const body = step.body.replace('{outside}', outside);
    let threadId;
    if (step.op === 'question') {
        const location = step.repository ? { kind: 'repository' } : fileLocation();
        threadId = (await store.createThread(review.id, location, body, reviewer, { intent: 'question' })).id;
        threads.set(step.name, threadId);
    } else {
        threadId = threads.get(step.thread);
        await store.addMessage(review.id, threadId, body, reviewer);
    }
    const answered = agentMessages(await thread(threadId)).length;
    if (step.decide !== undefined) {
        const waiting = await until(() => thread(threadId), t => t.agentState === 'waiting_for_human', 'a permission request');
        const request = agentMessages(waiting).at(-1).permission;
        if (step.decide) {
            decisions.resolve(request.id, step.decide);
        } else {
            await agents.cancel(review.id, threadId);
        }
    }
    if (step.stopAfter) {
        await until(() => thread(threadId), t => agentMessages(t).at(-1)?.body === step.stopAfter, 'the streamed text');
        await agents.cancel(review.id, threadId);
    }
    const done = await until(() => thread(threadId), t => agentMessages(t).length > answered && agentMessages(t).at(-1).status !== 'streaming' && t.agentState === 'idle', 'the answer');
    // The agent logs a message when it reads it; give a late `session/cancel` a moment.
    await sleep(100);
    const message = agentMessages(done).at(-1);
    const permission = message.permission && { ...message.permission, id: '{id}' };
    results.push(scrub({
        name: step.name,
        message: { body: message.body, status: message.status, activity: message.activity, permission },
        prompts: received().slice(before).filter(m => m.method === 'session/prompt').map(m => m.params)
    }));
}

const all = received();
const fixture = scrub({
    steps,
    results,
    initialize: all.find(m => m.method === 'initialize').params,
    sessionNew: all.filter(m => m.method === 'session/new').map(m => m.params),
    setConfigOption: all.filter(m => m.method === 'session/set_config_option').map(m => m.params),
    cancel: all.filter(m => m.method === 'session/cancel').map(m => m.params)
});
const out = join(crateRoot, 'tests/fixtures/acp_turns.json');
writeFileSync(out, JSON.stringify(fixture, undefined, 2) + '\n');
console.log(`wrote ${out} (${results.length} results)`);
agents['disconnect'](review.id);
rmSync(tmp, { recursive: true, force: true });
process.exit(0);
