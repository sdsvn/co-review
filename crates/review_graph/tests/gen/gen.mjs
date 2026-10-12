#!/usr/bin/env node
/**
 * Generates expected outputs from the TypeScript blastRadius and repositoryOverview functions
 * for the committed graph.json fixture, so the Rust tests can assert equality.
 *
 * Usage: node tests/gen/gen.mjs
 * Output: tests/fixtures/blast_radius_expected.json, tests/fixtures/overview_expected.txt
 */

import { createRequire } from 'module';
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

const repoRoot = join(__dirname, '..', '..', '..', '..');
const graphifyJs = join(repoRoot, 'extensions', 'review', 'lib', 'node', 'graphify.js');
const overviewJs = join(repoRoot, 'extensions', 'review', 'lib', 'node', 'repo-overview.js');
const fixtureDir = join(__dirname, '..', 'fixtures');

const graphJson = JSON.parse(readFileSync(join(fixtureDir, 'graph.json'), 'utf8'));
const { blastRadius } = require(graphifyJs);
const { repositoryOverview, DEPENDENCIES } = require(overviewJs);

// -- Blast radius --
// Changed lines: line 447 in review_model.rs (AgentPresence struct) and line 10 (top of file)
const changedLines = new Map([
  ['review_model/src/review_model.rs', [447, 10]],
]);

// blastRadius reads graph.json from disk, so we write the fixture temporarily.
// Instead, we'll call blastRadius inline by mocking the import path.
// Actually blastRadius reads from disk. Let's work around it:
// We create a temporary root with a graphify-out/graph.json in it.
import { mkdtempSync, cpSync } from 'fs';
import { tmpdir } from 'os';

const tempBase = mkdtempSync(join(tmpdir(), 'co-review-gen-'));
const tempRoot = join(tempBase, 'test-repo');
mkdirSync(tempRoot, { recursive: true });
mkdirSync(join(tempRoot, 'graphify-out'), { recursive: true });
writeFileSync(join(tempRoot, 'graphify-out', 'graph.json'), JSON.stringify(graphJson));

const br = await blastRadius(tempRoot, changedLines);
writeFileSync(join(fixtureDir, 'blast_radius_expected.json'), JSON.stringify(br, null, 2));
console.log(`blast_radius: ${br?.changed?.length ?? 0} changed, ${br?.affected?.length ?? 0} affected`);

// -- Overview (graph-based path) --
const overview = await repositoryOverview({
  root: tempRoot,
  coverage: { total: 5, viewed: 2, areas: [] },
  outlines: new Map(),
  graph: 'built',
});
writeFileSync(join(fixtureDir, 'overview_expected.txt'), overview);
console.log(`overview: ${overview.split('\n').length} lines`);

// -- Overview with non-empty coverage areas --
const overviewAreas = await repositoryOverview({
  root: tempRoot,
  coverage: {
    total: 12,
    viewed: 5,
    areas: [
      { path: 'src/api', total: 4, viewed: 2, next: 'src/api/handler.rs' },
      { path: 'src/db', total: 3, viewed: 1 },
    ],
  },
  outlines: new Map(),
  graph: 'built',
});
writeFileSync(join(fixtureDir, 'overview_with_areas_expected.txt'), overviewAreas);
console.log(`overview (with areas): ${overviewAreas.split('\n').length} lines`);

// -- Overview of a graph with edge cases: paths encodeURI changes, line numbers after a stray
// `L` or with a leading zero, and negative community ids (whose keys split oddly on '-').
writeFileSync(join(tempRoot, 'graphify-out', 'graph.json'), readFileSync(join(fixtureDir, 'edge_cases_graph.json'), 'utf8'));
const edgeCases = await repositoryOverview({
  root: tempRoot,
  coverage: { total: 15, viewed: 1, areas: [] },
  outlines: new Map(),
  graph: 'built',
});
writeFileSync(join(fixtureDir, 'edge_cases_overview_expected.txt'), edgeCases);
console.log(`overview (edge cases): ${edgeCases.split('\n').length} lines`);

// -- DEPENDENCIES set --
writeFileSync(join(fixtureDir, 'dependencies.json'), JSON.stringify([...DEPENDENCIES].sort()));

// Clean up
import { rmSync } from 'fs';
rmSync(tempBase, { recursive: true, force: true });

console.log('Done. Expected outputs written to tests/fixtures/');
