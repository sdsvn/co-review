// bin/patch-theia.mjs: the fixes to Theia are in node_modules, and applying them again changes nothing.
// Run: `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const PATCHED = [
    'node_modules/@theia/plugin-ext/lib/plugin/logger.js',
    'node_modules/@theia/plugin-ext/lib/hosted/node/plugin-host-logger.js',
    'node_modules/@theia/core/lib/node/messaging/websocket-frontend-connection-service.js'
];

test('every patch is applied, and applying them again is a no-op that succeeds', () => {
    const before = PATCHED.map(file => readFileSync(join(root, file), 'utf8'));
    for (const text of before) {
        assert.match(text, /co-review: see bin\/patch-theia\.mjs/);
    }
    const run = spawnSync(process.execPath, [join(root, 'bin', 'patch-theia.mjs')], { cwd: root, encoding: 'utf8' });
    assert.equal(run.status, 0, run.stderr);
    assert.equal(run.stdout, '', 'nothing left to patch');
    assert.deepEqual(PATCHED.map(file => readFileSync(join(root, file), 'utf8')), before);
});

test('an extension host warning about its own RPC never goes back through the RPC', () => {
    const text = readFileSync(join(root, PATCHED[1]), 'utf8');
    assert.match(text, /if \(formatted\.startsWith\('No reply handler for'\)\) \{ process\.stderr\.write/);
});
