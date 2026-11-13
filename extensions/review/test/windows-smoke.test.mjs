// Windows only (skipped elsewhere): runs what the unit tests can only describe. The `.cmd` shims installCli writes
// are executed through cmd.exe, a real npm .cmd shim (npm itself) runs through spawnCommand, arguments with spaces
// and cmd.exe meta characters survive the escaping, and killTree ends the program a .cmd started.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test); CI runs it on windows-latest.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const win = process.platform === 'win32';
const require = createRequire(import.meta.url);
require('reflect-metadata');
const lib = process.env.CO_REVIEW_LIB ?? '../lib';
const { AgentSetup } = require(`${lib}/node/agent-setup.js`);
const { findOnWindowsPath, runCommand, spawnCommand, killTree } = require(`${lib}/node/commands.js`);

function tmp(t) {
    const dir = mkdtempSync(join(tmpdir(), 'co-review-win-'));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    return dir;
}

test('the installed co-review.cmd runs the CLI through cmd.exe, found on PATH with PATHEXT', { skip: !win }, async t => {
    const dir = tmp(t);
    class Setup extends AgentSetup {
        get windowsBinDir() { return dir; }
    }
    const result = new Setup().installCliWindows();
    assert.match(result, /installed .*co-review\.cmd/);
    const path = process.env.PATH;
    process.env.PATH = `${dir};${path}`;
    t.after(() => { process.env.PATH = path; });
    assert.equal((findOnWindowsPath('co-review') ?? '').toLowerCase(), join(dir, 'co-review.cmd').toLowerCase());
    // A bare command name, as a harness would use it: resolved to the .cmd and run by cmd.exe.
    assert.match(await runCommand('co-review', ['version']), /^co-review \d+\.\d+\.\d+/);
    assert.match(await runCommand('co-review', ['--help']), /Usage: co-review/);
    const launch = JSON.parse(readFileSync(join(dir, 'launch.json'), 'utf8'));
    assert.match(await runCommand(launch.command, [...launch.args, 'version']), /^co-review \d+\.\d+\.\d+/, 'launch.json runs the CLI directly');
});

test('a real npm .cmd shim runs, and arguments with spaces and meta characters arrive intact', { skip: !win }, async t => {
    assert.match(findOnWindowsPath('npm') ?? '', /npm\.cmd$/i);
    assert.match(await runCommand('npm', ['--version']), /^\d+\.\d+\.\d+/);
    const dir = tmp(t);
    writeFileSync(join(dir, 'args.cmd'), '@echo off\r\nnode -e "console.log(JSON.stringify(process.argv.slice(1)))" %*\r\n');
    const args = ['C:\\Users\\Some User\\co review\\', 'a&b|c', 'say "hi"', '%PATH%', '(x) y'];
    const out = await runCommand(join(dir, 'args.cmd'), args);
    assert.deepEqual(JSON.parse(out), args);
});

test('killTree ends the program a .cmd started, not just cmd.exe', { skip: !win }, async t => {
    const dir = tmp(t);
    const pidFile = join(dir, 'pid');
    writeFileSync(join(dir, 'wait.cmd'), `@echo off\r\nnode -e "require('fs').writeFileSync(process.argv[1], String(process.pid)); setInterval(() => {}, 1000)" "${pidFile}"\r\n`);
    const child = spawnCommand(join(dir, 'wait.cmd'), [], { stdio: 'ignore' });
    for (let i = 0; i < 100 && !existsSync(pidFile); i++) {
        await new Promise(r => setTimeout(r, 100));
    }
    const pid = Number(readFileSync(pidFile, 'utf8'));
    assert.ok(pid > 0, 'the node child started');
    assert.doesNotThrow(() => process.kill(pid, 0), 'and is running');
    killTree(child);
    let alive = true;
    for (let i = 0; i < 50 && alive; i++) {
        await new Promise(r => setTimeout(r, 100));
        try {
            process.kill(pid, 0);
        } catch {
            alive = false;
        }
    }
    assert.equal(alive, false, 'the node child was ended with the tree');
});
