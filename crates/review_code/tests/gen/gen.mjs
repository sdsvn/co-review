#!/usr/bin/env node
/**
 * Records the TypeScript CodeFolders' decisions (compiled extensions/review/lib/node/code-folders.js) over a tiny
 * scenario, for crates/review_code/tests/conformance.rs to replay against the Rust port.
 *
 * The scenario (base and head file trees, with a modification in two hunks, a rename with an edit, a deletion and
 * new files) is written into the fixture, so the Rust test builds the same repository itself.
 *
 * Usage: node crates/review_code/tests/gen/gen.mjs
 * Output: crates/review_code/tests/fixtures/code_folders.json
 */
import { execFileSync } from 'child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'fs';
import { createRequire } from 'module';
import { tmpdir } from 'os';
import * as path from 'path';

const worktreeRoot = path.resolve(import.meta.dirname, '../../../..');
// The worktree is at <repo>/.claude/worktrees/<name>; the Theia dependencies are installed in the main checkout.
const repoRoot = path.resolve(worktreeRoot, '../../..');
const require = createRequire(path.join(repoRoot, 'node_modules', '_virtual.js'));

const scratch = mkdtempSync(path.join(tmpdir(), 'review-code-gen-'));
process.env.CO_REVIEW_HOME = path.join(scratch, 'home');
const { CodeFolders, sidesOf } = require(path.join(worktreeRoot, 'extensions/review/lib/node/code-folders.js'));

const appBase = Array.from({ length: 20 }, (_, i) => `base line ${i + 1}`);
const appHead = [...appBase];
appHead[1] = 'head line 2';
appHead.splice(18, 0, 'inserted line');
const scenario = {
    base: {
        'src/app.rs': appBase.join('\n') + '\n',
        'src/lib.rs': 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn sub(a: i32, b: i32) -> i32 {\n    a - b\n}\n',
        'README.md': '# Demo\n',
        'docs/guide.md': 'Guide\n',
    },
    head: {
        'src/app.rs': appHead.join('\n') + '\n',
        'src/library.rs': 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn sub(a: i32, b: i32) -> i32 {\n    b - a\n}\n',
        'src/utils/helper.rs': 'pub fn help() {}\n',
        'src/notes v2.txt': 'first\nsecond\n',
        'docs/guide.md': 'Guide\n',
    },
};

function git(cwd, ...args) {
    return execFileSync('git', ['-C', cwd, '-c', 'user.name=Gen', '-c', 'user.email=gen@example.com', '-c', 'commit.gpgsign=false', ...args],
        { encoding: 'utf8' });
}

function writeTree(root, files) {
    for (const [file, text] of Object.entries(files)) {
        mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
        writeFileSync(path.join(root, file), text);
    }
}

const repo = path.join(scratch, 'repo');
mkdirSync(repo);
git(repo, 'init', '--quiet', '-b', 'main');
writeTree(repo, scenario.base);
git(repo, 'add', '-A');
git(repo, 'commit', '--quiet', '-m', 'base');
git(repo, 'checkout', '--quiet', '-b', 'feature');
for (const file of Object.keys(scenario.base)) {
    rmSync(path.join(repo, file));
}
writeTree(repo, scenario.head);
git(repo, 'add', '-A');
git(repo, 'commit', '--quiet', '-m', 'head');
git(repo, 'checkout', '--quiet', 'main');
const patch = git(repo, 'diff', 'main...feature');
const base = git(repo, 'rev-parse', 'main').trim();

const dir = path.join(scratch, 'review-dir');
mkdirSync(dir);
writeFileSync(path.join(dir, 'change.patch'), patch);

// Only what onDiff and baseFile use: the review's bundle, and the review directory's patches.
const code = path.join(scratch, 'code');
const folders = new CodeFolders();
folders.bundles = { patchesOf: d => [path.join(d, 'change.patch')] };
const review = { id: 'r1', bundle: { dir, code, repo, base } };
const uriOf = file => require('@theia/core/lib/common/file-uri').FileUri.create(path.join(code, file)).toString();
const at = (start, end) => ({ start: { line: start - 1, character: 0 }, end: { line: end - 1, character: 0 } });

// Locations as the editor gives them: `file` relative to the code folder (or a full `uri`), one-based lines.
const locations = [
    { name: 'added line', file: 'src/app.rs', lines: [2, 2], text: 'head line 2' },
    { name: 'context line', file: 'src/app.rs', lines: [3, 3], text: 'base line 3' },
    { name: 'context and added range', file: 'src/app.rs', lines: [2, 4], text: 'head line 2\nbase line 3\nbase line 4' },
    { name: 'range with shorter text', file: 'src/app.rs', lines: [2, 3], text: 'head line 2' },
    { name: 'second hunk inserted line', file: 'src/app.rs', lines: [19, 19], text: 'inserted line' },
    { name: 'line between hunks', file: 'src/app.rs', lines: [10, 10], text: 'base line 10' },
    { name: 'range across the gap', file: 'src/app.rs', lines: [5, 16] },
    { name: 'edited since the diff', file: 'src/app.rs', lines: [2, 2], text: 'edited line 2' },
    { name: 'no anchor text', file: 'src/app.rs', lines: [2, 2] },
    { name: 'renamed file', file: 'src/library.rs', lines: [6, 6], text: '    b - a' },
    { name: 'old path of a renamed file', file: 'src/lib.rs', lines: [6, 6], text: '    a - b' },
    { name: 'new file', file: 'src/utils/helper.rs', lines: [1, 1], text: 'pub fn help() {}' },
    { name: 'new file with a space', file: 'src/notes v2.txt', lines: [1, 2], text: 'first\nsecond' },
    { name: 'deleted file', file: 'README.md', lines: [1, 1], text: '# Demo' },
    { name: 'unchanged file', file: 'docs/guide.md', lines: [1, 1], text: 'Guide' },
    { name: 'outside the code folder', uri: 'file:///elsewhere/src/app.rs', lines: [2, 2], text: 'head line 2' },
    { name: 'not a file', uri: 'untitled:Untitled-1', lines: [2, 2], text: 'head line 2' },
    { name: 'no range', file: 'src/app.rs', text: 'head line 2' },
];

const onDiff = [];
for (const entry of locations) {
    const location = { kind: entry.lines && entry.lines[0] !== entry.lines[1] ? 'range' : 'line', uri: entry.uri ?? uriOf(entry.file) };
    if (entry.lines) {
        location.range = at(...entry.lines);
    }
    if (entry.text !== undefined) {
        location.anchor = { text: entry.text };
    }
    const result = await folders.onDiff(review, location);
    if (result) {
        // The patch's URI depends on where the review directory is; the test compares the file name.
        result.uri = path.basename(new URL(result.uri).pathname);
    }
    onDiff.push({ ...entry, result: result ?? null });
}

const baseFiles = [];
for (const file of ['src/app.rs', 'src/lib.rs', 'README.md', 'src/library.rs', 'src/utils/helper.rs', '../outside', '/etc/hosts', 'src/../README.md']) {
    baseFiles.push({ file, result: (await folders.baseFile(review, file)) ?? null });
}

const ranges = ['main...feature', 'main..feature', '...feature', 'main...', '..', 'HEAD~1', 'a...b...c', 'a..b..c', 'a....b', ''];
const sides = ranges.map(range => ({ range, result: sidesOf(range) }));

const out = path.join(import.meta.dirname, '../fixtures/code_folders.json');
mkdirSync(path.dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify({ scenario, patch, onDiff, baseFiles, sides }, null, 2) + '\n');
rmSync(scratch, { recursive: true, force: true });
console.log(`wrote ${out}: ${onDiff.length} onDiff cases, ${baseFiles.length} base reads, ${sides.length} ranges`);
