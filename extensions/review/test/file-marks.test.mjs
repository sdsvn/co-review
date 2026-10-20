// Marking files viewed or "don't care" (src/node/review-store.ts), and finding a file on the patch page (src/common/patch.ts).
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const require = createRequire(import.meta.url);
const lib = process.env.CO_REVIEW_LIB ?? '../lib';
process.env.CO_REVIEW_HOME = mkdtempSync(join(tmpdir(), 'co-review-marks-'));
const { ReviewStore } = require(`${lib}/node/review-store.js`);
const { matchesPath } = require(`${lib}/common/patch.js`);

test('a file is viewed, "don\'t care" or neither, never both', async () => {
    const store = new ReviewStore();
    const repo = mkdtempSync(join(tmpdir(), 'co-review-repo-'));
    const review = await store.create({ workspaceRoot: `file://${repo}`, title: 'Marks', scope: { kind: 'repository' }, author: { id: 'me', name: 'Me', kind: 'human' } });
    let r = await store.setViewed(review.id, ['src/a.ts', 'src/b.ts'], 'skipped');
    assert.deepEqual(Object.keys(r.skipped).sort(), ['src/a.ts', 'src/b.ts']);
    r = await store.setViewed(review.id, ['src/a.ts'], true);
    assert.deepEqual(Object.keys(r.viewed), ['src/a.ts']);
    assert.deepEqual(Object.keys(r.skipped), ['src/b.ts']);
    r = await store.setViewed(review.id, ['src/a.ts', 'src/b.ts'], false);
    assert.deepEqual([Object.keys(r.viewed), Object.keys(r.skipped)], [[], []]);
});

test('finding a file matches its path\'s characters in order, ignoring case and spaces', () => {
    assert.ok(matchesPath('prw', 'browser/patch/patch-review-widget.tsx'));
    assert.ok(matchesPath('Patch Widget', 'browser/patch/patch-review-widget.tsx'));
    assert.ok(matchesPath('', 'anything'));
    assert.ok(!matchesPath('wp', 'patch'));
    assert.ok(!matchesPath('store.go', 'internal/db/db.go'));
});
