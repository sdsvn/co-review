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

test('a change review lists its files as a folder tree, and puts generated and version-only files on one line', () => {
    const row = (t, s) => ({ t, s });
    const patches = [{ name: 'pr-1', files: [
        { path: 'src/a.ts', added: 3, removed: 1, rows: [row('add', 'const a = 1;')] },
        { path: 'src/b.ts', added: 1, removed: 0, rows: [row('add', 'export {}')] },
        { path: 'README.md', added: 2, removed: 2, rows: [row('add', 'Hello')] },
        { path: 'package-lock.json', added: 5, removed: 5, rows: [row('add', '"lodash": "4.17.21",')] },
        { path: 'app/package.json', added: 1, removed: 1, rows: [row('del', '  "version": "0.6.5",'), row('add', '  "version": "0.7.0",')] }
    ] }];
    const page = overviewPage(change, '/repo', { patches, radius: 'no graph' });
    assert.match(page, /5 files, \+12 −9\.\n\n- \*\*\(root\)\*\* 1 file · \+2 −2\n  - `README\.md` \+2 −2\n- \*\*src\/\*\* 2 files · \+4 −1\n  - `src\/a\.ts` \+3 −1\n  - `src\/b\.ts` \+1 −0/);
    assert.match(page, /Also changed, generated or version only \(2\): `package-lock\.json`, `app\/package\.json`\./);
    assert.doesNotMatch(page, /first pass by|Accept or dismiss/);
});

test('the blast radius names a few affected definitions per area, not every changed one', () => {
    const node = (id, area, hops = 1) => ({ id, label: id, file: `src/${id}.ts`, line: 1, area, hops, direction: 'uses it' });
    const radius = {
        changed: [node('a'), node('b'), node('c')],
        affected: [node('x1', 'Server'), node('x2', 'Server'), node('x3', 'Server'), node('x4', 'Server', 2), node('y1', 'Client')],
        edges: [], files: []
    };
    const page = overviewPage(change, '/repo', { patches: [{ name: 'pr-1', files: [] }], radius });
    assert.match(page, /The change touches 3 definitions in 3 files\./);
    assert.doesNotMatch(page, /\*\*Changed\*\*|\[`a`\]/);
    assert.match(page, /- \*\*Server\*\* \(4\): \[`x1`\]\(src\/x1\.ts:1\), \[`x2`\]\(src\/x2\.ts:1\), \[`x3`\]\(src\/x3\.ts:1\), and 1 more\n- \*\*Client\*\* \(1\)/);
});
