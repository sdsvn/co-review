// The overview page (src/node/overview-page.ts): what it says while its slow section is still being built.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { overviewPage } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/overview-page.js` : '../lib/node/overview-page.js');

const base = {
    id: 'r1', title: 'A review', workspaceRoot: 'file:///repo', scope: { kind: 'repository' },
    participants: [], threads: [], activity: [], nextThreadNumber: 1, createdAt: 0
};
const change = { ...base, bundle: { dir: '/reviews/pr-1', repo: '/repo', code: '/repo' } };

test('a repository review says the repository is being indexed until the section is ready', () => {
    const building = overviewPage(base, '/repo', { patches: [], building: true });
    assert.match(building, /## The repository\n\n_Co-Review is indexing the repository/);
    const ready = overviewPage(base, '/repo', { patches: [], repository: '# Overview\n\nStart in src/.' });
    assert.match(ready, /## The repository\n\nStart in src\/\./);
    assert.doesNotMatch(ready, /being built|is indexing/);
    const none = overviewPage(base, '/repo', { patches: [] });
    assert.doesNotMatch(none, /## The repository/);
});

test('a change review says the code graph is being built until the blast radius is ready', () => {
    const patches = [{ name: 'pr-1', files: [{ path: 'src/a.ts', added: 1, removed: 0 }] }];
    const building = overviewPage(change, '/repo', { patches, building: true });
    assert.match(building, /## Blast radius\n\n_Co-Review is building the code graph/);
    assert.doesNotMatch(building, /is indexing the repository/);
    const none = overviewPage(change, '/repo', { patches, radius: 'no graph' });
    assert.match(none, /## Blast radius\n\n_With \[Graphify\]/);
    assert.doesNotMatch(none, /building the code graph/);
});
