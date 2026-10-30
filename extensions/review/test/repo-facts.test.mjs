// The repository facts (src/node/repo-facts.ts): what a guide to the repository is written from, computed without a model.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { computeFacts, parseLog, words } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/repo-facts.js` : '../lib/node/repo-facts.js');

const file = (symbols, more = {}) => ({ mtimeMs: 0, size: 100, lines: 50, symbols: symbols.map(([n, m]) => [n, m ?? []]), kinds: symbols.map(([, , k]) => k ?? 'function'), ...more });
const head = { sha: 'abc1234', date: '2026-10-30', dirty: false };

// A small service: a model everyone imports, a store and service using it, an API registering routes, a CLI, tests,
// generated code, and three adapters with the same files.
const files = {
    'src/model/user.ts': file([['User', [], 'interface'], ['UserId', [], 'type'], ['Order', [], 'interface']]),
    'src/model/order.ts': file([['OrderStatus', [], 'enum'], ['OrderLine', [], 'interface']]),
    'src/store/user-store.ts': file([['UserStore', ['getUser', 'saveUser'], 'class']], { imports: ['../model/user'], env: ['DATABASE_URL'] }),
    'src/store/order-store.ts': file([['OrderStore', ['getOrder', 'saveOrder'], 'class']], { imports: ['../model/order', '../model/user'] }),
    'src/service/user-service.ts': file([['UserService', ['createUser'], 'class']], { imports: ['../store/user-store', '../model/user', 'lodash'] }),
    'src/api/routes.ts': file([['registerRoutes']], {
        imports: ['../service/user-service', 'express'],
        calls: [['reg', 'app.get', '/users', 3], ['reg', 'app.post', '/users', 4], ['reg', 'app.get', '/orders', 5], ['reg', 'app.get', '/health', 6], ['out', 'fetch', '', 9]]
    }),
    'src/cli.ts': file([['main']], { imports: ['./service/user-service'], calls: [['out', 'execFile', 'git', 2]] }),
    'src/gen/api.gen.ts': file([['ApiClient', [], 'class']], { generated: true }),
    'src/gen/types.gen.ts': file([['ApiTypes', [], 'type']], { generated: true }),
    'adapters/alpha/client.ts': file([['AlphaClient', [], 'class']]),
    'adapters/alpha/token.ts': file([['alphaToken']]),
    'adapters/beta/client.ts': file([['BetaClient', [], 'class']]),
    'adapters/beta/token.ts': file([['betaToken']]),
    'adapters/gamma/client.ts': file([['GammaClient', [], 'class']]),
    'adapters/gamma/token.ts': file([['gammaToken']]),
    'test/user-store.test.ts': file([['testSaveUser']], { imports: ['../src/store/user-store'], calls: [['reg', 'app.get', '/fake', 1]] })
};
const commit = (sha, subject, paths, added = []) => ({ sha, date: '2026-10-01', author: 'Ada', subject, files: paths.map(p => ({ path: p, changed: 10 })), added });
const commits = [
    commit('c1', 'Orders', ['src/store/order-store.ts', 'src/model/order.ts', 'src/api/routes.ts'], ['src/store/order-store.ts']),
    commit('c2', 'Users', ['src/store/user-store.ts', 'src/model/user.ts', 'src/api/routes.ts'], ['src/store/user-store.ts']),
    commit('c3', 'Fix', ['src/store/user-store.ts', 'src/model/user.ts']),
    commit('c4', 'Fix', ['src/store/user-store.ts', 'src/model/user.ts']),
    commit('c5', 'Reformat', Array.from({ length: 60 }, (_, i) => `x/${i}.ts`))
];
const facts = computeFacts({ files, head, commits, totalCommits: 5, manifests: { 'package.json': JSON.stringify({ name: 'svc', bin: { svc: 'src/cli.ts' }, scripts: { test: 'node --test' } }) } });

test('identifiers split into words the way people read them', () => {
    assert.deepEqual(words('UserStoreImpl'), ['user', 'store', 'impl']);
    assert.deepEqual(words('HTTPServer'), ['http', 'server']);
    assert.deepEqual(words('user_store-x'), ['user', 'store', 'x']);
});

test('entry points come from manifests, main functions and registration calls, not from tests', () => {
    const points = facts.entry.points.map(p => `${p.kind}:${p.name}`);
    assert.ok(points.includes('bin:svc') && points.includes('script:test'));
    assert.ok(points.includes('main:src/cli.ts'));
    assert.ok(points.includes('registration:/users') && points.includes('registration:/orders'));
    assert.ok(!points.includes('registration:/fake'), 'a test registering a route is not an entry point');
    assert.deepEqual(facts.entry.outbound.map(o => o.target).sort(), ['fetch', 'git']);
    assert.deepEqual(facts.entry.env, [{ name: 'DATABASE_URL', files: ['src/store/user-store.ts'] }]);
    assert.deepEqual(facts.entry.external.map(e => e.module).sort(), ['express', 'lodash']);
});

test('folders get a role with a reason, and generated ones are on the ignore line', () => {
    const role = Object.fromEntries(facts.folders.map.map(f => [f.path, f.role]));
    assert.equal(role['src/gen'], 'generated');
    assert.equal(role['test'], 'tests');
    assert.equal(role['src/model'], 'core', 'imported by everything, all types');
    assert.equal(role['src/api'], 'glue', 'nothing imports it; it imports the service');
    assert.deepEqual(facts.folders.ignore, ['src/gen']);
    const model = facts.folders.map.find(f => f.path === 'src/model');
    assert.match(model.why, /used by \d+ files outside/);
    assert.ok(model.usedBy.includes('src/store'));
});

test('the most imported file is the most load-bearing', () => {
    assert.equal(facts.load[0].path, 'src/model/user.ts');
    assert.equal(facts.load[0].importers, 3);
});

test('naming: casing rules, look-alike names and repeated shapes', () => {
    assert.ok(facts.names.casing.some(r => r.kind === 'type' && r.form === 'PascalCase'));
    const stems = facts.names.lookalikes.map(g => g.stem);
    assert.ok(stems.includes('Order'), 'Order, OrderStatus, OrderStore and OrderLine are told apart');
    assert.deepEqual(facts.shapes.siblingSets, [{ parent: 'adapters', files: ['client.ts', 'token.ts'], folders: ['adapters/alpha', 'adapters/beta', 'adapters/gamma'] }]);
    assert.equal(facts.shapes.registrations[0].via, 'app.get');
});

test('history: large commits are ignored, coupling and recipes come from what changes together', () => {
    assert.equal(facts.history.thin, true);
    assert.equal(facts.history.ignoredLarge, 1);
    assert.deepEqual(facts.history.coupling.map(c => `${c.a}+${c.b}`), ['src/model/user.ts+src/store/user-store.ts']);
    assert.equal(facts.history.recipes[0].adds, 'src/store/*.ts');
    assert.equal(facts.history.recipes[0].times, 2);
    assert.deepEqual(facts.history.recipes[0].alsoTouched, ['src/api/routes.ts']);
});

test('a focus narrows every section to one folder', () => {
    const focused = computeFacts({ files, head, commits, totalCommits: 5, manifests: {}, focus: 'src/store' });
    assert.equal(focused.files, 2);
    assert.ok(focused.folders.map.every(f => f.path.startsWith('src/store')));
});

test('git log output is parsed with renames and created files', () => {
    const log = '\x1eaaa\x1f2026-10-01\x1fAda\x1fMove it\n\n3\t1\tsrc/{old => new}/a.ts\n-\t-\timg.png\n';
    const [c] = parseLog(log, '\x1eaaa\n\nsrc/new/a.ts\n');
    assert.deepEqual(c.files, [{ path: 'src/new/a.ts', changed: 4 }, { path: 'img.png', changed: 0 }]);
    assert.deepEqual(c.added, ['src/new/a.ts']);
});
