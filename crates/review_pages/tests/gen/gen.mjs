#!/usr/bin/env node
/**
 * Records the Markdown the TypeScript app writes for synthetic reviews of every kind, so the Rust
 * port can assert byte equality.
 *
 * Runs the compiled TypeScript (`extensions/review/lib/node/overview-page.js`, `repo-overview.js`;
 * build the extension first). Usage: node crates/review_pages/tests/gen/gen.mjs
 *
 * Output: tests/fixtures/overview_cases.json (overviewPage), tests/fixtures/repository_cases.json
 * (repositoryOverview: the repo map fallback and the graph dispatch).
 */
import { createRequire } from 'module';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const lib = resolve(here, '../../../../extensions/review/lib/node');
const { overviewPage } = require(join(lib, 'overview-page.js'));
const { repositoryOverview } = require(join(lib, 'repo-overview.js'));
const fixtures = resolve(here, '../fixtures');
mkdirSync(fixtures, { recursive: true });

// ---------------------------------------------------------------------------------------------
// Synthetic reviews
// ---------------------------------------------------------------------------------------------

const human = { id: 'human:ann@example.com', kind: 'human', name: 'Ann' };
const agentParticipant = { id: 'agent:claude', kind: 'agent', name: 'Claude' };
let messageId = 0;
const message = (author, body) => ({ id: `m${++messageId}`, author, body, createdAt: 1 });

function review(fields) {
    return {
        id: 'r1', title: 'Synthetic review', workspaceRoot: 'file:///work/repo', scope: { kind: 'repository' },
        participants: [human], threads: [], activity: [], nextThreadNumber: 1, createdAt: 1, updatedAt: 1,
        ...fields
    };
}

function thread(number, location, messages, fields = {}) {
    return {
        id: `t${number}`, number, location, messages, status: 'open', intent: 'comment', createdAt: 1, updatedAt: 1,
        ...fields
    };
}

const range = (start, end) => ({ start: { line: start, character: 0 }, end: { line: end, character: 4 } });
const longCode = Array.from({ length: 15 }, (_, i) => `line ${i} has \`\`\` ticks`).join('\n') + '\n\n';

const everyThread = [
    thread(3, { kind: 'range', uri: 'file:///work/repo/src/api/handler.ts', range: range(9, 14), symbol: 'handle',
        anchor: { text: longCode } },
    [message(agentParticipant, '## Missing **error** handling\n\nThe handler drops errors.\n# A heading in a message'),
        message(human, '  '), message(human, 'Agreed.\nLet us fix it.')],
    { status: 'proposed', severity: 'high', labels: ['API'] }),
    thread(1, { kind: 'line', uri: 'file:///work/repo/src/db/store.ts', range: range(4, 4), anchor: { text: '   const x = 1;   ' } },
        [message(human, 'Why a constant here?')], { severity: 'low' }),
    thread(2, { kind: 'file', uri: 'file:///work/repo/README.md' }, [message(human, 'x'.repeat(120))],
        { status: 'resolved', severity: 'medium' }),
    thread(4, { kind: 'repository' }, [message(human, '### Overall the structure is fine')]),
    thread(5, { kind: 'file', uri: 'file:///elsewhere/other.ts' }, [message(human, 'Outside the root')], { status: 'resolved' }),
    thread(6, { kind: 'file', uri: 'file:///work/repo/src/db/schema%20v2.sql', range: range(0, 0) },
        [message(human, 'Encoded path, emoji 😀😀😀 ' + 'é'.repeat(80))], { labels: ['Database'] }),
    thread(7, { kind: 'document', uri: 'file:///home/.co-review/workspaces/abc/pages/r1/overview.md' },
        [message(human, 'A comment on the overview page itself')]),
    thread(8, { kind: 'directory', uri: 'file:///work/repo/src' }, []),
    thread(9, { kind: 'line', uri: 'file:///work/repo/src/api/routes.ts', range: range(1, 1) },
        [message(agentParticipant, 'Second in API')], { status: 'proposed', labels: ['API'] }),
];

const radius = {
    changed: [
        { id: 'a', label: 'handle`Request`', file: 'src/api/handler.ts', line: 10, area: 'API' },
        { id: 'b', label: 'Store "main"', file: 'src/db/store.ts' }
    ],
    affected: [
        { id: 'c', label: 'router', file: 'src/api/routes.ts', line: 3, area: 'API', hops: 1, direction: 'uses it' },
        { id: 'd', label: 'main', file: 'src/main.ts', line: 1, hops: 2, direction: 'uses it' },
        { id: 'e', label: 'query', file: 'src/db/query.ts', line: 7, area: 'Database', hops: 1, direction: 'it uses' },
        { id: 'f', label: 'log', file: 'src/log.ts', hops: 1, direction: 'it uses' }
    ],
    edges: [{ from: 'c', to: 'a' }, { from: 'd', to: 'c' }, { from: 'a', to: 'e' }, { from: 'b', to: 'f' }, { from: 'x', to: 'a' }],
    files: ['src/api/routes.ts', 'src/main.ts', 'src/db/query.ts', 'src/log.ts']
};
const wideRadius = {
    changed: Array.from({ length: 20 }, (_, i) => ({ id: `c${i}`, label: `changed${i}`, file: `src/c${i}.ts`, line: i, area: 'Core' })),
    affected: Array.from({ length: 20 }, (_, i) => ({ id: `a${i}`, label: `affected${i}`, file: `src/a${i}.ts`, line: i + 1,
        area: i % 2 ? 'Odd' : undefined, hops: 1 + (i % 2), direction: i % 3 ? 'uses it' : 'it uses' })),
    edges: Array.from({ length: 50 }, (_, i) => ({ from: `a${i % 20}`, to: `c${(i * 7) % 20}` })),
    files: []
};

const patches = [{ name: 'change', files: [
    { path: 'src/api/handler.ts', added: 12, removed: 3 }, { path: 'src/db/store.ts', added: 1, removed: 0 }] }];
const twoPatches = [{ name: 'part-1', files: [{ path: 'a.ts', added: 1, removed: 1 }] },
    { name: 'part-2', files: [{ path: 'b.ts', added: 0, removed: 5 }, { path: 'c.ts', added: 2, removed: 0 }] }];
const bundle = { dir: '/work/reviews/change', repo: '/work/repo' };
const patchThreads = [
    thread(1, { kind: 'patch', uri: 'file:///work/reviews/change/change.patch',
        patchAnchor: { type: 'code-line', path: 'src/api/handler.ts', line: 12 } },
        [message(agentParticipant, 'Off by one')], { status: 'proposed', severity: 'high' }),
    thread(2, { kind: 'patch', uri: 'file:///work/reviews/change/change.patch', patchAnchor: { type: 'patch' } },
        [message(human, 'Whole patch comment')]),
    thread(3, { kind: 'patch', patchAnchor: { type: 'code-range', path: 'src/db/store.ts', startLine: 4, endLine: 9 } },
        [message(human, 'Range in the store')], { labels: ['Storage'] }),
    thread(4, { kind: 'document', uri: 'file:///work/reviews/change/PR.md',
        docAnchor: { type: 'text', exact: 'This pull request rewrites the handler and is a long sentence over sixty characters' } },
        [message(human, 'On the PR text')]),
    thread(5, { kind: 'document', uri: 'file:///work/reviews/change/design.md',
        docAnchor: { type: 'mermaid-node', blockId: 'flow', nodeId: 'B' } }, [message(human, 'On a diagram node')],
        { status: 'resolved' }),
];

const overviewText = '# The change\n\nIt rewrites the handler.\n\n## How it works\n\n```mermaid\n%% id: flow\nflowchart LR\n  A --> B\n```\n\n'
    + '````md\n```\n# not a heading\n```\n## still in the outer fence\n````\n\n### Risks\n###### Six hashes stay\n#NoSpace\n';
const repositoryText = '# Repository overview: repo\n\nBuilt from somewhere.\n\n## Start here\n\n- [main](src/main.ts)\n\n## Areas\n\n~~~\n## fenced\n~~~\n';
const prBody = '# Rewrite the handler\n\nThe handler now returns errors.\n\n## Testing\n\n- unit tests\n';

const cases = [
    ['repository, building', review({}), '/work/repo', { patches: [], building: true }],
    ['repository, built', review({ overview: '  \n ' }), '/work/repo', { patches: [], repository: repositoryText }],
    ['repository, empty section', review({}), '/work/repo', { patches: [], repository: '' }],
    ['repository, every thread', review({ threads: everyThread, agent: { id: 'claude', name: 'Claude' },
        verdict: { decision: 'request-changes', summary: 's', submittedAt: 1, count: 2 }, overview: overviewText }),
    '/work/repo', { patches: [], repository: repositoryText }],
    ['repository, one thread, approved', review({ threads: [everyThread[1]],
        verdict: { decision: 'approve', summary: '', submittedAt: 1, count: 1 } }), '/work/repo/', { patches: [] }],
    ['repository, no findings, comment verdict', review({ agent: { id: 'codex', name: 'Codex' },
        verdict: { decision: 'comment', summary: '', submittedAt: 1, count: 3 } }), '/work/repo', { patches: [], building: true }],
    ['change, radius', review({ bundle, threads: patchThreads, agent: { id: 'claude', name: 'Claude' }, overview: overviewText }),
        bundle.dir, { patches, radius }],
    ['change, wide radius', review({ bundle }), bundle.dir, { patches, radius: wideRadius }],
    ['change, no graph, two patches', review({ bundle }), bundle.dir, { patches: twoPatches, radius: 'no graph' }],
    ['change, radius without definitions', review({ bundle }), bundle.dir,
        { patches, radius: { changed: [], affected: [], edges: [], files: [] } }],
    ['change, radius without affected', review({ bundle }), bundle.dir,
        { patches, radius: { changed: radius.changed, affected: [], edges: [], files: [] } }],
    ['change, radius without edges', review({ bundle }), bundle.dir,
        { patches, radius: { ...radius, edges: [{ from: 'zz', to: 'a' }] } }],
    ['change, building', review({ bundle }), bundle.dir, { patches, building: true }],
    ['change without repository, building', review({ bundle: { dir: bundle.dir } }), bundle.dir, { patches, building: true }],
    ['change, no patches', review({ bundle }), bundle.dir, { patches: [] }],
    ['pull request, full', review({ bundle, title: 'PR #42: Rewrite', threads: patchThreads }), bundle.dir,
        { pr: { title: 'Rewrite the handler', number: 42, branch: 'feature/x', base: 'main', commits: 3,
            url: 'https://github.com/o/r/pull/42', body: prBody }, patches, radius: 'no graph' }],
    ['pull request, number only', review({ bundle }), bundle.dir,
        { pr: { number: 7, url: 'https://github.com/o/r/pull/7', commits: 1, body: '' }, patches }],
    ['pull request, title only', review({ bundle }), bundle.dir,
        { pr: { title: 'Local branch', branch: 'topic', commits: 0, body: '  \n' }, patches }],
    ['pull request, url only', review({ bundle }), bundle.dir, { pr: { url: 'https://example.com/pr', body: 'Body only' }, patches }],
];

const overviewCases = cases.map(([name, r, root, parts]) => ({ name, review: r, root, parts, expected: overviewPage(r, root, parts) }));
writeFileSync(join(fixtures, 'overview_cases.json'), JSON.stringify(overviewCases, null, 1) + '\n');

// ---------------------------------------------------------------------------------------------
// Repository overview: the repo map fallback, and the dispatch to the graph
// ---------------------------------------------------------------------------------------------

const outlines = [
    ['README.md', ''],
    ['build.rs', 'main'],
    ['cmd/tool/main.go', 'main, run, parseFlags'],
    ['docs/guide.md', ''],
    ['index.ts', 'start'],
    ['lib/[weird] name.ts', 'Weird { a, b }'],
    ['src/api/handler.ts', 'Handler { handle, validate, respond, log, retry, close, open, a, b, c, d, e, … }, createHandler, helper'],
    ['src/api/routes.ts', 'routes, register'],
    ['src/app.TSX', 'App { render }'],
    ['src/db/store.ts', 'Store { get, set, delete }, connect, disconnect, migrate, seed'],
    ['src/server.js', ''],
    ['src/cli.py', 'main, parse'],
    ['src/ünïcode/naïve.rs', 'Naive, other'],
    ['tools/x.rb', 'X { y }, Z'],
];
const coverage = {
    total: 14, viewed: 3, areas: [
        { path: '.', total: 3, viewed: 1, next: 'build.rs' },
        { path: 'cmd', total: 1, viewed: 1 },
        { path: 'docs', total: 1, viewed: 0, next: 'docs/guide.md' },
        { path: 'lib', total: 1, viewed: 0, next: 'lib/[weird] name.ts' },
        { path: 'src', total: 7, viewed: 1, next: 'src/ünïcode/naïve.rs' },
        { path: 'tools', total: 1, viewed: 0 },
        { path: 'empty', total: 0, viewed: 0 },
    ]
};
const smallGraph = {
    nodes: [
        { id: 'a', label: 'handle', source_file: 'src/api/handler.ts', source_location: 'L10', community: 0, community_name: 'API' },
        { id: 'b', label: 'routes', source_file: 'src/api/routes.ts', source_location: 'L1', community: 0, community_name: 'API' },
        { id: 'c', label: 'register', source_file: 'src/api/routes.ts', source_location: 'L5', community: 0, community_name: 'API' },
        { id: 'd', label: 'Store', source_file: 'src/db/store.ts', source_location: 'L3', community: 1, community_name: 'Database' },
        { id: 'e', label: 'connect', source_file: 'src/db/store.ts', source_location: 'L20', community: 1, community_name: 'Database' },
        { id: 'f', label: 'migrate', source_file: 'src/db/store.ts', source_location: 'L40', community: 1, community_name: 'Database' },
    ],
    links: [
        { source: 'a', target: 'd', relation: 'calls' }, { source: 'b', target: 'a', relation: 'calls' },
        { source: 'c', target: 'e', relation: 'calls' }, { source: 'e', target: 'f', relation: 'calls' },
        { source: 'b', target: 'c', relation: 'contains' },
    ]
};
const repositoryCases = [
    ['map with readme', 'with-readme', { readme: 'README.md' }, outlines, coverage, undefined],
    ['map, lowercase readme, graph missing', 'lower-readme', { readme: 'readme.md' }, outlines, coverage, 'missing'],
    ['map, no readme, graph failed', 'no-readme', {}, outlines, coverage, 'failed'],
    ['map, empty repository', 'empty', {}, [], { total: 0, viewed: 0, areas: [] }, 'built'],
    ['map, graph without definitions', 'empty-graph', { graph: { nodes: [], links: [] } }, outlines, coverage, 'built'],
    ['graph', 'graph-repo', { graph: smallGraph }, outlines, coverage, 'built'],
];
const base = mkdtempSync(join(tmpdir(), 'co-review-pages-'));
const repositoryResults = [];
for (const [name, directory, files, caseOutlines, caseCoverage, graph] of repositoryCases) {
    const root = join(base, directory);
    mkdirSync(root, { recursive: true });
    if (files.readme) {
        writeFileSync(join(root, files.readme), '# Readme\n');
    }
    const graphText = files.graphText ?? (files.graph ? JSON.stringify(files.graph) : undefined);
    if (graphText) {
        mkdirSync(join(root, 'graphify-out'), { recursive: true });
        writeFileSync(join(root, 'graphify-out', 'graph.json'), graphText);
    }
    const expected = await repositoryOverview({ root, coverage: caseCoverage, outlines: new Map(caseOutlines), graph });
    repositoryResults.push({ name, directory, readme: files.readme, graphText, outlines: caseOutlines, coverage: caseCoverage, graph, expected });
}
rmSync(base, { recursive: true, force: true });
writeFileSync(join(fixtures, 'repository_cases.json'), JSON.stringify(repositoryResults, null, 1) + '\n');

console.log(`${overviewCases.length} overview cases, ${repositoryResults.length} repository cases`);
