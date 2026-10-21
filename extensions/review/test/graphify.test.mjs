// Finding a repository's Graphify graphs (src/node/graphify.ts): per-folder graphs, and the main checkout's for a worktree.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const require = createRequire(import.meta.url);
const { findGraphs, loadGraph, blastRadius } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/graphify.js` : '../lib/node/graphify.js');

function graph(dir, json) {
    mkdirSync(join(dir, 'graphify-out'), { recursive: true });
    writeFileSync(join(dir, 'graphify-out', 'graph.json'), JSON.stringify(json));
}

test('per-folder graphs are found in the repository and, from a worktree, in its main checkout', async t => {
    const base = realpathSync(mkdtempSync(join(tmpdir(), 'co-review-graphify-')));
    t.after(() => rmSync(base, { recursive: true, force: true }));
    const repo = join(base, 'repo');
    mkdirSync(join(repo, 'services', 'a'), { recursive: true });
    mkdirSync(join(repo, 'pkg', 'b'), { recursive: true });
    writeFileSync(join(repo, 'services', 'a', 'x.go'), 'x');
    const git = (...args) => execFileSync('git', ['-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t', ...args]);
    git('init', '-q');
    git('add', '.');
    git('commit', '-qm', 'init');
    git('worktree', 'add', '-q', join(base, 'wt'));
    // Each graph's paths are relative to its folder, and both use the id `f`.
    graph(join(repo, 'services'), {
        nodes: [{ id: 'f', label: 'F', source_file: 'a/x.go', source_location: 'L1', community: 0 },
            { id: 'g', label: 'G', source_file: 'a/x.go', source_location: 'L5', community: 0 }],
        links: [{ source: 'g', target: 'f', relation: 'calls' }]
    });
    graph(join(repo, 'pkg'), { nodes: [{ id: 'f', label: 'H', source_file: 'b/y.go', source_location: 'L1', community: 0 }], links: [] });

    assert.deepEqual((await findGraphs(repo)).map(g => g.scope), ['pkg', 'services']);
    assert.deepEqual((await findGraphs(join(base, 'wt'))).map(g => g.file), [join(repo, 'pkg', 'graphify-out', 'graph.json'), join(repo, 'services', 'graphify-out', 'graph.json')]);

    const { graph: merged } = await loadGraph(join(base, 'wt'));
    assert.deepEqual(merged.nodes.map(n => [n.id, n.source_file, n.community]),
        [['pkg:f', 'pkg/b/y.go', 0], ['services:f', 'services/a/x.go', 1], ['services:g', 'services/a/x.go', 1]]);
    const radius = await blastRadius(join(base, 'wt'), new Map([['services/a/x.go', [1]]]));
    assert.deepEqual(radius.affected.map(n => [n.label, n.direction]), [['G', 'uses it']]);
});

test('a repository without graphs has none', async t => {
    const dir = mkdtempSync(join(tmpdir(), 'co-review-graphify-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    assert.deepEqual(await findGraphs(dir), []);
    assert.equal(await loadGraph(dir), undefined);
});
