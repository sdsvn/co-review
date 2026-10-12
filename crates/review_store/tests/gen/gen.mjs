#!/usr/bin/env node
/**
 * Drives the compiled TypeScript ReviewStore with a sequence of operations,
 * recording the resulting files for comparison with the Rust implementation.
 *
 * Usage: CO_REVIEW_HOME=<tmpdir> node gen.mjs
 *
 * Output: JSON files under $CO_REVIEW_HOME, plus a manifest at $CO_REVIEW_HOME/manifest.json
 * listing each operation and the review state after it.
 */

import { createRequire } from 'module';
import { createHash } from 'crypto';
import { promises as fs } from 'fs';
import * as path from 'path';

// gen.mjs is at crates/review_store/tests/gen/gen.mjs — four levels up is the worktree root
const worktreeRoot = path.resolve(import.meta.dirname, '../../../..');
// The worktree is at <repo>/.claude/worktrees/<name>, so the main repo is three levels up
const repoRoot = path.resolve(worktreeRoot, '../../..');

// Use createRequire to load CJS modules with the repo's node_modules on the path
const require = createRequire(path.join(repoRoot, 'node_modules', '_virtual.js'));

// The compiled store imports from @theia/core and uses @injectable() from inversify.
// Load it using require with the correct module resolution.
const { ReviewStore, coReviewHome } = require(
    path.join(worktreeRoot, 'extensions/review/lib/node/review-store.js')
);

const home = process.env.CO_REVIEW_HOME;
if (!home) {
    console.error('Set CO_REVIEW_HOME to a temp directory');
    process.exit(1);
}

const store = new ReviewStore();
const manifest = [];

function recordStep(name, data) {
    manifest.push({ step: name, ...data });
}

const author = { id: 'human:test@test.com', kind: 'human', name: 'Test User' };
// A path that doesn't exist anywhere, so real_path / realPath returns it as-is
const workspaceRoot = 'file:///nonexistent_test_workspace_42/project';

// Record the workspace hash before operations
const wsDir = store.workspaceDir(workspaceRoot);
const wsHash = path.basename(wsDir);
console.log(`Workspace hash: ${wsHash}`);

// 1. Create a review
const review = await store.create({
    workspaceRoot,
    title: 'Test Review',
    scope: { kind: 'repository' },
    author,
});
recordStep('create', { reviewId: review.id, title: review.title });

// 2. List reviews
const listed = await store.list(workspaceRoot);
recordStep('list', { count: listed.length });

// 3. Rename
const renamed = await store.rename(review.id, 'Renamed Review');
recordStep('rename', { title: renamed.title });

// 4. Create a thread
const thread = await store.createThread(
    review.id,
    { kind: 'repository' },
    'First comment',
    author,
);
recordStep('createThread', { threadId: thread.id, number: thread.number });

// 5. Add a message
const thread2 = await store.addMessage(review.id, thread.id, 'Reply message', author);
recordStep('addMessage', { messageCount: thread2.messages.length });

// 6. Set thread status
await store.setThreadStatus(review.id, thread.id, 'resolved', author);
recordStep('setThreadStatus', { status: 'resolved' });

// 7. Archive
const archived = await store.archive(review.id, true);
recordStep('archive', { hasArchivedAt: archived.archivedAt !== undefined });

// 8. Unarchive
const unarchived = await store.archive(review.id, false);
recordStep('unarchive', { archivedAt: unarchived.archivedAt });

// 9. Set viewed
await store.setViewed(review.id, ['src/main.rs', 'src/lib.rs'], true);
recordStep('setViewed', {});

// 10. Submit
const submitted = await store.submit(review.id, 'approve', 'LGTM', author);
recordStep('submit', { decision: submitted.verdict.decision, count: submitted.verdict.count });

// 11. Set overview
await store.setOverview(review.id, '# Overview\nThis is a test.');
recordStep('setOverview', {});

// Read the final review state
const finalReview = await store.get(review.id);

// Write manifest
await fs.writeFile(
    path.join(home, 'manifest.json'),
    JSON.stringify({ wsHash, steps: manifest, finalReview }, null, 2),
);

console.log(`Done. ${manifest.length} steps recorded.`);
console.log(`Review file: ${path.join(wsDir, 'reviews', review.id + '.json')}`);
