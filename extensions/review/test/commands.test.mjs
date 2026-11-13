// Running harness and agent commands on Windows (src/node/commands.ts): finding a command the way cmd.exe does
// (PATHEXT), and running a .cmd shim through cmd.exe with cross-spawn's escaping. Pure functions with the platform
// and environment passed in, so they run here. Run after `tsc` (lib/): `node --test extensions/review/test`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { findOnWindowsPath, windowsCommandLine, windowsSpawn } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/commands.js` : '../lib/node/commands.js');

const files = new Set([
    'C:\\Users\\dev\\AppData\\Roaming\\npm\\npx.cmd',
    'C:\\Users\\dev\\AppData\\Roaming\\npm\\npx',          // npm's bash shim: never picked
    'C:\\Users\\dev\\AppData\\Roaming\\npm\\gemini.cmd',
    'C:\\Program Files\\Git\\cmd\\git.exe',
    'C:\\tools\\claude.exe'
]);
const env = { PATH: 'C:\\Windows\\system32;C:\\Program Files\\Git\\cmd;C:\\Users\\dev\\AppData\\Roaming\\npm;C:\\tools', PATHEXT: '.COM;.EXE;.BAT;.CMD', ComSpec: 'C:\\Windows\\system32\\cmd.exe' };
// Windows file names are case-insensitive, and PATHEXT is upper case.
const lower = new Set([...files].map(f => f.toLowerCase()));
const lookup = { env, exists: f => lower.has(f.toLowerCase()) };

test('findOnWindowsPath appends PATHEXT and skips the extensionless shim', () => {
    assert.equal(findOnWindowsPath('npx', lookup), 'C:\\Users\\dev\\AppData\\Roaming\\npm\\npx.CMD');
    assert.equal(findOnWindowsPath('git', lookup), 'C:\\Program Files\\Git\\cmd\\git.EXE');
    assert.equal(findOnWindowsPath('claude', lookup), 'C:\\tools\\claude.EXE');
    assert.equal(findOnWindowsPath('gemini.cmd', lookup), 'C:\\Users\\dev\\AppData\\Roaming\\npm\\gemini.cmd');
    assert.equal(findOnWindowsPath('opencode', lookup), undefined);
    assert.equal(findOnWindowsPath('C:\\tools\\claude', lookup), 'C:\\tools\\claude.EXE', 'a path is looked up in place');
    assert.equal(findOnWindowsPath('npx', { ...lookup, env: { PATH: 'C:\\nowhere' } }), undefined);
    // Never the current directory (cmd.exe would): a repository under review may contain an npx.cmd.
    assert.equal(findOnWindowsPath('npx', { ...lookup, exists: f => f.toLowerCase() === 'npx.cmd' }), undefined);
    assert.equal(findOnWindowsPath('.\\npx', { ...lookup, exists: f => f.toLowerCase() === '.\\npx.cmd' }), '.\\npx.CMD', 'unless asked for by path');
});

test('windowsSpawn runs an .exe directly and a .cmd through cmd.exe /d /s /c', () => {
    assert.deepEqual(windowsSpawn('claude', ['mcp', 'add'], lookup), { file: 'C:\\tools\\claude.EXE', args: ['mcp', 'add'], batch: false });
    const w = windowsSpawn('npx', ['-y', '@agentclientprotocol/claude-agent-acp'], lookup);
    assert.equal(w.batch, true);
    assert.equal(w.file, 'C:\\Windows\\system32\\cmd.exe');
    assert.deepEqual(w.args.slice(0, 3), ['/d', '/s', '/c']);
    // Expected strings computed with cross-spawn's own escape (command + argument(arg, true)).
    assert.equal(w.args[3], '"C:\\Users\\dev\\AppData\\Roaming\\npm\\npx.CMD ^^^"-y^^^" ^^^"@agentclientprotocol/claude-agent-acp^^^""');
    // Unknown command: spawned as given, so the error names it.
    assert.deepEqual(windowsSpawn('opencode', ['acp'], lookup), { file: 'opencode', args: ['acp'], batch: false });
});

test('windowsCommandLine escapes like cross-spawn: quotes, trailing backslashes, meta characters, twice for cmd.exe', () => {
    // A repository path with spaces and a trailing backslash, and an argument with quotes and & | characters.
    assert.equal(windowsCommandLine('C:\\Program Files\\x\\pi.cmd', ['install', 'C:\\Users\\Some User\\co-review\\']),
        '"C:\\Program^ Files\\x\\pi.cmd ^^^"install^^^" ^^^"C:\\Users\\Some^^^ User\\co-review\\\\^^^""');
    assert.equal(windowsCommandLine('t.cmd', ['say "hi" & more|less']),
        '"t.cmd ^^^"say^^^ \\^^^"hi\\^^^"^^^ ^^^&^^^ more^^^|less^^^""');
});
