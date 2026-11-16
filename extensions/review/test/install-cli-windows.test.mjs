// The co-review command on Windows (AgentSetup.installCliWindows, src/node/agent-setup.ts): with no `sh` there,
// the app writes `.cmd` shims itself instead of running bin/install-cli.sh, and finds them again afterwards.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const electron = !!process.versions.electron;
const escape = s => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const require = createRequire(import.meta.url);
require('reflect-metadata');
const { AgentSetup } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/agent-setup.js` : '../lib/node/agent-setup.js');

function fixture(t) {
    const dir = mkdtempSync(join(tmpdir(), 'co-review-cli-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    class Setup extends AgentSetup {
        get windowsBinDir() { return dir; }
    }
    return { dir, agent: new Setup() };
}

test('installCliWindows writes co-review.cmd and co-review-server.cmd running the bundled scripts', t => {
    const { dir, agent } = fixture(t);
    const path = process.env.PATH;
    process.env.PATH = '/usr/bin';
    t.after(() => { process.env.PATH = path; });
    const result = agent.installCliWindows();
    for (const [cmd, script] of [['co-review', 'co-review.mjs'], ['co-review-server', 'review-server.mjs']]) {
        const file = join(dir, `${cmd}.cmd`);
        assert.ok(existsSync(file), `${file} exists`);
        const lines = readFileSync(file, 'utf8').split('\r\n');
        assert.equal(lines[0], '@echo off');
        assert.equal(lines[1], 'setlocal');
        // On node the shim runs node; on the app's Electron (CI also runs this with ELECTRON_RUN_AS_NODE) it runs
        // that Electron as Node, which needs ELECTRON_RUN_AS_NODE set first.
        const run = new RegExp(`^${electron ? `set "ELECTRON_RUN_AS_NODE=1"\r\n"${escape(process.execPath)}"` : 'node'} ".*[\\\\/]bin[\\\\/]${script}" %\\*$`);
        assert.match(lines.slice(2, -1).join('\r\n'), run);
        assert.equal(lines[lines.length - 1], '', 'ends with a CRLF');
        assert.ok(result.includes(`installed ${file}`));
    }
    assert.match(result, /not on your PATH/);
    // What the shims run, for the Pi / Oh My Pi packages, which cannot spawn a .cmd: node, or the app's Electron.
    const launch = JSON.parse(readFileSync(join(dir, 'launch.json'), 'utf8'));
    assert.equal(launch.command, electron ? process.execPath : 'node');
    assert.match(launch.args[0], /[\\/]bin[\\/]co-review\.mjs$/);
    assert.deepEqual(launch.env, electron ? { ELECTRON_RUN_AS_NODE: '1' } : {});
});

test('installCliWindows says nothing about PATH when the directory is on it, and installedCli finds the command', t => {
    const { dir, agent } = fixture(t);
    const path = process.env.PATH;
    process.env.PATH = `C:\\Windows;${dir.toUpperCase()}\\`;
    t.after(() => { process.env.PATH = path; });
    const result = agent.installCliWindows();
    assert.doesNotMatch(result, /PATH/);
    const platform = Object.getOwnPropertyDescriptor(process, 'platform');
    Object.defineProperty(process, 'platform', { value: 'win32' });
    t.after(() => Object.defineProperty(process, 'platform', platform));
    assert.equal(agent.installedCli, join(dir, 'co-review.cmd'));
    // Harnesses still get the runtime, not the .cmd: spawned without a shell, a .cmd does not run on Windows.
    assert.equal(agent.launch().command, process.execPath);
    assert.ok(agent.info().cliInstalled);
});
