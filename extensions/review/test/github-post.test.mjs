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

const { parsePatch } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/common/patch.js` : '../lib/common/patch.js');

test('inside a hunk, "--- x" is a removed "-- x" and "+++ x" an added "++ x"; after it, nothing is a diff line', () => {
    const [file, ...rest] = parsePatch([
        'diff --git a/q.sql b/q.sql', '--- a/q.sql', '+++ b/q.sql',
        '@@ -1,4 +1,4 @@', ' select 1;', '--- old note', '+++ new note', ' select 2;', '-select 3;', '+select 4;',
        '-- ', '2.40.0', ''
    ].join('\n'));
    assert.equal(rest.length, 0);
    assert.equal(file.path, 'q.sql');
    assert.deepEqual(file.rows.slice(1).map(r => [r.t, r.o, r.n, r.s]), [
        ['ctx', 1, 1, 'select 1;'], ['del', 2, undefined, '-- old note'], ['add', undefined, 2, '++ new note'],
        ['ctx', 3, 3, 'select 2;'], ['del', 4, undefined, 'select 3;'], ['add', undefined, 4, 'select 4;']
    ]);
});

test('a commit message and diffstat between two commits\' diffs are not rows of the first', () => {
    const files = parsePatch([
        'diff --git a/a.go b/a.go', '--- a/a.go', '+++ b/a.go', '@@ -1 +1 @@', '-x', '+y',
        'From 123 Mon Sep 17 00:00:00 2001', 'Subject: [PATCH 2/2] more', '', '- fix the thing', '---', ' c.go | 2 +-', '',
        'diff --git a/c.go b/c.go', '--- a/c.go', '+++ b/c.go', '@@ -1 +1 @@', '-p', '+q'
    ].join('\n'));
    assert.deepEqual(files.map(f => [f.path, f.rows.length]), [['a.go', 3], ['c.go', 3]]);
});

const edited = (proposal, source) => new GitHubReviews().compose(review([{
    ...finding(1, [say(human, 'Rename it.')]), proposal: { status: 'pending', ...proposal },
    location: { kind: 'patch', uri: 'file:///reviews/pr-1/pr-1.patch', patchAnchor: { type: 'code-line', path: 'src/a.go', line: 1, source } }
}]), 'comment', '').comments[0].body;

test('a suggested edit of the comment\'s whole lines is a GitHub suggestion', () => {
    assert.match(edited({ before: 'x := 1', after: 'y := 1' }, 'x := 1'), /```suggestion\ny := 1\n```$/);
});

test('an edit of part of a line is a diff, never a suggestion that would replace the whole line', () => {
    const body = edited({ before: 'x', after: 'y' }, 'x := 1');
    assert.doesNotMatch(body, /suggestion/);
    assert.match(body, /```diff\n-x\n\+y\n```$/);
});

test('a comment on the change\'s code checkout goes on its lines in the pull request', () => {
    const r = review([{
        ...finding(1, [say(human, 'Why?')]),
        location: { kind: 'range', uri: 'file:///work/pr-1/src/b.go', range: { start: { line: 9, character: 0 }, end: { line: 11, character: 4 } } }
    }]);
    r.bundle = { dir: '/reviews/pr-1', code: '/work/pr-1', head: 'abc' };
    const { comments } = new GitHubReviews().compose(r, 'comment', '');
    assert.deepEqual(comments.map(({ path, line, start_line, side }) => ({ path, line, start_line, side })),
        [{ path: 'src/b.go', line: 12, start_line: 10, side: 'RIGHT' }]);
});

test('only the comments off the pull request\'s diff go into the body, with their place', async () => {
    class Fake extends GitHubReviews {
        async gh() {
            // `--jq '... | tojson'`: one file's JSON per line.
            return { ok: true, stderr: '', stdout: JSON.stringify({ filename: 'src/a.go', patch: '@@ -1,2 +1,3 @@\n a\n+b\n c\n@@ -10,1 +11,1 @@\n-d\n+e' }) + '\n' };
        }
    }
    const comment = (line, start_line) => ({ threadId: `t${line}`, path: 'src/a.go', line, start_line, side: 'RIGHT', body: `on ${line}` });
    const { comments, off } = await new Fake().fit({ repo: 'o/r', number: 1 }, [comment(2), comment(11, 3), comment(50)]);
    assert.deepEqual(comments.map(c => c.line), [2]);
    assert.deepEqual(off.map(c => c.line), [11, 50]);
});
