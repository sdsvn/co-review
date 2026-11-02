// What a review posts to GitHub (src/node/github.ts): each thread as one comment of the reviewer's.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
require('reflect-metadata');
const { GitHubReviews } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/github.js` : '../lib/node/github.js');

const human = { id: 'h', name: 'Reviewer', kind: 'human' };
const agent = { id: 'a', name: 'claude-code', kind: 'agent' };
const say = (author, body, extra = {}) => ({ id: body, author, body, createdAt: 0, ...extra });
const finding = (number, messages) => ({
    id: `t${number}`, number, status: 'open', origin: 'finding', messages,
    location: { kind: 'patch', uri: 'file:///reviews/pr-1/pr-1.patch', patchAnchor: { type: 'code-line', path: 'src/a.go', line: number } }
});
const review = threads => ({
    id: 'r1', title: 'PR', workspaceRoot: 'file:///repo', scope: { kind: 'repository' },
    participants: [], threads, activity: [], nextThreadNumber: threads.length + 1, createdAt: 0
});
const bodies = threads => new GitHubReviews().compose(review(threads), 'comment', '').comments.map(c => c.body);

test('a reviewer\'s question goes out with the agent\'s answer, not on its own', () => {
    const [body] = bodies([finding(1, [
        say(agent, 'The precheck renders templates for every item.'),
        say(human, 'What can be done?'),
        say(agent, 'Precheck only the items that will become new versions.')
    ])]);
    assert.equal(body, 'The precheck renders templates for every item.\n\nWhat can be done?\n\nPrecheck only the items that will become new versions.');
});

test('the change the agent spelled out stays between the reviewer\'s messages', () => {
    const [body] = bodies([finding(1, [
        say(agent, 'Headers are validated raw.'),
        say(human, 'The headers must be normalized first'),
        say(agent, 'Canonicalise them in parseHeaders, before validate().'),
        say(human, 'Yes, headers must be canonicalised before any validation')
    ])]);
    assert.match(body, /normalized first\n\nCanonicalise them in parseHeaders, before validate\(\)\.\n\nYes, headers/);
});

test('an agent\'s reworded comment replaces the thread; unfinished agent messages and permission prompts never go out', () => {
    const thread = finding(1, [
        say(agent, 'Finding.'),
        say(human, 'Why?'),
        say(agent, 'Half an answ', { status: 'streaming' }),
        say(agent, 'Post this?', { permission: { id: 'p', title: 'Post?', options: [] } })
    ]);
    assert.equal(bodies([thread])[0], 'Finding.\n\nWhy?');
    const reworded = new GitHubReviews().compose(review([thread]), 'comment', '', new Map([['t1', 'Do X.']])).comments[0].body;
    assert.equal(reworded, 'Do X.');
});

test('an agent\'s reply before the reviewer joined is not posted with their point', () => {
    const [body] = bodies([finding(1, [say(agent, 'Finding.'), say(agent, 'More detail.'), say(human, 'Agreed.')])]);
    assert.equal(body, 'Finding.\n\nAgreed.');
});
