// The review loop's store side (src/node/review-store.ts): an agent's answer, findings added again, a round picked up;
// and which occurrence of a quote a tracked page comment is (src/common/anchor-tracking.ts).
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const require = createRequire(import.meta.url);
const lib = process.env.CO_REVIEW_LIB ?? '../lib';
process.env.CO_REVIEW_HOME = mkdtempSync(join(tmpdir(), 'co-review-flow-'));
const { ReviewStore } = require(`${lib}/node/review-store.js`);
const { occurrenceOnPage } = require(`${lib}/common/anchor-tracking.js`);

const me = { id: 'me', name: 'Me', kind: 'human' };
const agent = { id: 'agent:a', name: 'Agent', kind: 'agent' };

async function reviewWithAgent() {
    const store = new ReviewStore();
    const repo = mkdtempSync(join(tmpdir(), 'co-review-repo-'));
    const review = await store.create({ workspaceRoot: `file://${repo}`, title: 'Flow', scope: { kind: 'repository' }, author: me });
    await store.setAgent(review.id, { id: agent.id, name: agent.name, transport: 'mcp' });
    return { store, review };
}

test('an answer goes after the message the agent was given, before a follow-up it was not', async () => {
    const { store, review } = await reviewWithAgent();
    const thread = await store.createThread(review.id, { kind: 'repository' }, 'Why?', me, { intent: 'question' });
    const asked = thread.messages[0].id;
    await store.addMessage(review.id, thread.id, 'And also this?', me);
    const { thread: answered, pending } = await store.answer(review.id, thread.id, 'Because.', agent, id => id === asked);
    assert.equal(pending, true);
    assert.deepEqual(answered.messages.map(m => m.body), ['Why?', 'Because.', 'And also this?']);
    assert.equal(answered.agentState, 'queued');
});

test('an answer with nothing new from the reviewer is the last message, and the thread is idle', async () => {
    const { store, review } = await reviewWithAgent();
    const thread = await store.createThread(review.id, { kind: 'repository' }, 'Why?', me, { intent: 'question' });
    const asked = thread.messages[0].id;
    const { thread: answered, pending } = await store.answer(review.id, thread.id, 'Because.', agent, id => id === asked);
    assert.equal(pending, false);
    assert.deepEqual(answered.messages.map(m => m.body), ['Why?', 'Because.']);
    assert.equal(answered.agentState, 'idle');
});

test('a finding added again (same id) is the thread it already is', async () => {
    const { store, review } = await reviewWithAgent();
    const add = () => store.createThread(review.id, { kind: 'repository' }, 'A finding', agent, { status: 'proposed', origin: 'finding', sourceId: 'f1' });
    const [a, b] = await Promise.all([add(), add()]);
    assert.equal(a.id, b.id);
    assert.equal((await store.get(review.id)).threads.length, 1);
});

test('a round is marked delivered only if it is still the latest', async () => {
    const { store, review } = await reviewWithAgent();
    await store.submit(review.id, 'comment', 'Round 1', me);
    await store.submit(review.id, 'comment', 'Round 2', me);
    let r = await store.verdictDelivered(review.id, 1);
    assert.notEqual(r.verdict.toAgent, 'delivered');
    r = await store.verdictDelivered(review.id, 2);
    assert.equal(r.verdict.toAgent, 'delivered');
});

test('a quote\'s occurrence counts only what the page shows', () => {
    const source = [
        '---', 'title: Refund flow', '---',
        '```mermaid', 'flowchart LR', '  A[Refund] --> B', '```',
        'See [the guide](docs/Refund.md).',
        '<!-- Refund notes -->',
        'A Refund is issued here.',
        'A Refund is checked there.'
    ].join('\n');
    const first = source.indexOf('Refund is issued');
    const second = source.indexOf('Refund is checked');
    assert.equal(occurrenceOnPage(source, 'Refund', first), 0);
    assert.equal(occurrenceOnPage(source, 'Refund', second), 1);
});
