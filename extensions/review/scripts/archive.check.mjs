// Runnable check for review archiving: node scripts/archive.check.mjs
// Exercises the compiled store against a throwaway CO_REVIEW_HOME.
import { mkdtempSync, rmSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import assert from 'assert';

const home = mkdtempSync(join(tmpdir(), 'co-review-archive-'));
process.env.CO_REVIEW_HOME = home;

const { ReviewStore } = await import('../lib/node/review-store.js');

try {
    const store = new ReviewStore();
    const root = 'file:///tmp/repo';
    const author = { id: 'u1', name: 'Tester', kind: 'human' };

    const review = await store.create({ workspaceRoot: root, title: 'A review', scope: { kind: 'repository' }, author });
    assert.ok(!review.archivedAt, 'new review is not archived');
    assert.equal((await store.list(root)).length, 1, 'one review listed');

    const archived = await store.archive(review.id, true);
    assert.ok(archived.archivedAt > 0, 'archive sets archivedAt');
    const reload = await store.get(review.id);
    assert.ok(reload.archivedAt > 0, 'archivedAt persists to disk');
    // The store still holds it (filtering is the caller's job); active/mobile lists filter on archivedAt.
    assert.equal((await store.list(root)).filter(r => !r.archivedAt).length, 0, 'archived hidden from active list');

    const unarchived = await store.archive(review.id, false);
    assert.equal(unarchived.archivedAt, undefined, 'unarchive clears archivedAt');
    assert.equal((await store.list(root)).filter(r => !r.archivedAt).length, 1, 'unarchived back in active list');

    console.log('archive.check: OK');
} finally {
    rmSync(home, { recursive: true, force: true });
}
