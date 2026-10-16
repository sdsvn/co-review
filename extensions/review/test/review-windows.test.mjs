// The review windows (ReviewWindows, src/node/agent-coordination.ts): every review an agent shows has a window of its
// own, so two reviews of one repository are open at once and closing one is told only to its agents.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
require('reflect-metadata');
const { ReviewWindows } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/agent-coordination.js` : '../lib/node/agent-coordination.js');

const repo = 'file:///repo';

function windows(t) {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const w = new ReviewWindows();
    const events = [];
    w.onDidOpen(e => events.push(['open', e.reviewId ?? '-']));
    w.onDidClose(e => events.push(['close', e.reviewId ?? '-', e.by]));
    w.onDidRequestClose(e => events.push(['request-close', e.reviewId ?? '-']));
    return { w, events };
}

test('two reviews of one repository are two windows, and closing one closes only it', t => {
    const { w, events } = windows(t);
    const a = {}, b = {};
    w.attach({ workspaceRoot: repo, reviewId: 'A' }, a);
    w.attach({ workspaceRoot: repo, reviewId: 'B' }, b);
    assert.deepEqual(events, [['open', 'A'], ['open', 'B']]);
    assert.ok(w.hasWindowOf('A') && w.hasWindowOf('B'));

    w.detach(a);
    t.mock.timers.tick(4000);
    assert.deepEqual(events.slice(2), [['close', 'A', 'reviewer']]);
    assert.ok(!w.hasWindowOf('A') && w.hasWindowOf('B'));
});

test('a reload within the grace period is neither closed nor opened again', t => {
    const { w, events } = windows(t);
    const first = {}, reloaded = {};
    w.attach({ workspaceRoot: repo, reviewId: 'A' }, first);
    w.detach(first);
    t.mock.timers.tick(1000);
    w.attach({ workspaceRoot: repo, reviewId: 'A' }, reloaded);
    t.mock.timers.tick(10_000);
    assert.deepEqual(events, [['open', 'A']]);
});

test('a repository window and a review window of it are separate', t => {
    const { w, events } = windows(t);
    const repoWindow = {}, reviewWindow = {};
    w.attach({ workspaceRoot: repo }, repoWindow);
    w.attach({ workspaceRoot: repo, reviewId: 'A' }, reviewWindow);
    w.detach(repoWindow);
    t.mock.timers.tick(4000);
    assert.deepEqual(events, [['open', '-'], ['open', 'A'], ['close', '-', 'reviewer']]);
    assert.ok(w.hasWindowOf('A'));
});

test('an agent done with its review asks only that review\'s window to close', t => {
    const { w, events } = windows(t);
    // shown() only tracks windows of an app an agent started.
    Object.defineProperty(w, 'startedByAgent', { value: true });
    w.shown({ workspaceRoot: repo, reviewId: 'A' }, 'session-1');
    w.shown({ workspaceRoot: repo, reviewId: 'B' }, 'session-2');
    w.done('session-1', 'done', 'B');
    assert.deepEqual(events, []);
    w.done('session-1', 'done', 'A');
    assert.deepEqual(events, [['request-close', 'A']]);
    w.done('session-2', 'ended');
    assert.deepEqual(events, [['request-close', 'A'], ['request-close', 'B']]);
});
