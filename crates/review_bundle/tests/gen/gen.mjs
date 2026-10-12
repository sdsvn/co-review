#!/usr/bin/env node
/**
 * Generate test fixtures for review_bundle by running the TypeScript parsers
 * on real patches from this repository's git history plus hand-crafted edge cases.
 *
 * Output: crates/review_bundle/tests/fixtures/patch_fixtures.json
 *         crates/review_bundle/tests/fixtures/meta_fixtures.json
 *         crates/review_bundle/tests/fixtures/discovery_fixtures.json
 *         crates/review_bundle/tests/fixtures/findings_fixtures.json
 */
import { execSync } from 'child_process';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, resolve } from 'path';
import { fileURLToPath } from 'url';
import { createRequire } from 'module';

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

const { parsePatch, parsePatchMeta, changedLines, DOCUMENT_NAMES } = require(
    resolve(__dirname, '../../../../extensions/review/lib/common/patch.js')
);

const fixturesDir = resolve(__dirname, '../fixtures');
mkdirSync(fixturesDir, { recursive: true });

const repoRoot = resolve(__dirname, '../../../..');

function git(args) {
    return execSync(`git ${args}`, { cwd: repoRoot, maxBuffer: 10 * 1024 * 1024 }).toString();
}

// --- Patch fixtures ---

const patchFixtures = [];

function addPatchFixture(name, input) {
    const files = parsePatch(input);
    const cl = changedLines(files);
    // Convert Map to plain object for JSON
    const changedLinesObj = {};
    for (const [k, v] of cl) {
        changedLinesObj[k] = v;
    }
    patchFixtures.push({ name, input, expected: { files, changedLines: changedLinesObj } });
}

// 1. Real patches from git history
const commits = git('log --oneline -60').trim().split('\n').map(l => l.split(' ')[0]);
for (let i = 0; i < Math.min(commits.length, 50); i++) {
    const sha = commits[i];
    try {
        const diff = git(`diff ${sha}~1..${sha} 2>/dev/null`);
        if (diff.trim()) {
            addPatchFixture(`git-diff-${sha}`, diff);
        }
    } catch { /* merge commit or root */ }
}

// 2. format-patch for a few commits
for (let i = 0; i < Math.min(commits.length, 15); i++) {
    const sha = commits[i];
    try {
        const patch = git(`format-patch -1 ${sha} --stdout 2>/dev/null`);
        if (patch.trim()) {
            addPatchFixture(`format-patch-${sha}`, patch);
        }
    } catch { /* skip */ }
}

// 3. Hand-crafted edge cases

// Empty diff
addPatchFixture('empty', '');

// Simple add-only
addPatchFixture('simple-add', `diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,3 @@
+line 1
+line 2
+line 3
`);

// Simple delete
addPatchFixture('simple-delete', `diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1,3 +0,0 @@
-line 1
-line 2
-line 3
`);

// Rename
addPatchFixture('rename', `diff --git a/old-name.txt b/new-name.txt
similarity index 100%
rename from old-name.txt
rename to new-name.txt
`);

// Rename with changes
addPatchFixture('rename-with-changes', `diff --git a/old-name.txt b/new-name.txt
similarity index 80%
rename from old-name.txt
rename to new-name.txt
--- a/old-name.txt
+++ b/new-name.txt
@@ -1,3 +1,3 @@
 kept
-old
+new
 also kept
`);

// Binary file
addPatchFixture('binary', `diff --git a/image.png b/image.png
new file mode 100644
Binary files /dev/null and b/image.png differ
`);

// Mode change
addPatchFixture('mode-change', `diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
`);

// No newline at end of file
addPatchFixture('no-newline-eof', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 line 1
-line 2
+line 2 modified
 line 3
\\ No newline at end of file
`);

// CRLF input
addPatchFixture('crlf', "diff --git a/file.txt b/file.txt\r\n--- a/file.txt\r\n+++ b/file.txt\r\n@@ -1,3 +1,3 @@\r\n hello\r\n-world\r\n+universe\r\n end\r\n");

// Path with spaces
addPatchFixture('path-with-spaces', `diff --git "a/path with spaces/file.txt" "b/path with spaces/file.txt"
--- "a/path with spaces/file.txt"
+++ "b/path with spaces/file.txt"
@@ -1 +1 @@
-old
+new
`);

// Unicode path
addPatchFixture('unicode-path', `diff --git a/src/café.txt b/src/café.txt
--- a/src/café.txt
+++ b/src/café.txt
@@ -1 +1 @@
-bonjour
+salut
`);

// Multiple files
addPatchFixture('multiple-files', `diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-old a
+new a
diff --git a/b.txt b/b.txt
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-old b
+new b
diff --git a/c.txt b/c.txt
new file mode 100644
--- /dev/null
+++ b/c.txt
@@ -0,0 +1 @@
+new file
`);

// Empty hunk (file with only metadata, no actual diff content)
addPatchFixture('empty-hunk-file', `diff --git a/empty.txt b/empty.txt
--- a/empty.txt
+++ b/empty.txt
`);

// Multiple hunks
addPatchFixture('multiple-hunks', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,5 +1,5 @@
 line 1
-line 2
+line 2 changed
 line 3
 line 4
 line 5
@@ -20,5 +20,5 @@
 line 20
-line 21
+line 21 changed
 line 22
 line 23
 line 24
`);

// Plain unified diff (no diff --git header)
addPatchFixture('plain-unified', `--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 hello
-world
+universe
 end
`);

// Hunk header with function name
addPatchFixture('hunk-with-function', `diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,5 +10,6 @@ fn main() {
     let x = 1;
-    let y = 2;
+    let y = 3;
+    let z = 4;
     println!("{}", x);
     return;
`);

// Only removed lines (tests changedLines: removed lines point to the next kept/added line)
addPatchFixture('only-removed', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,5 +1,3 @@
 kept 1
-removed a
-removed b
 kept 2
 kept 3
`);

// Context line after removed lines (tests changedLines)
addPatchFixture('del-then-ctx', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,4 +1,3 @@
 keep
-gone
 still here
 end
`);

// Consecutive add and del
addPatchFixture('add-del-interleaved', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,4 +1,4 @@
-old1
+new1
-old2
+new2
 kept
 end
`);

// Trailing empty line (important: the TS parser treats it as context)
addPatchFixture('trailing-empty-in-hunk', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 line 1
-line 2
+line 2 mod
 line 3
`);

// Diff with /dev/null source (new file via plain diff)
addPatchFixture('devnull-new', `diff --git a/brand-new.txt b/brand-new.txt
new file mode 100644
--- /dev/null
+++ b/brand-new.txt
@@ -0,0 +1,2 @@
+hello
+world
`);

// Diff with /dev/null target (deleted file)
addPatchFixture('devnull-deleted', `diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-goodbye
-world
`);

// Hunk starting at line 0 (empty old file)
addPatchFixture('hunk-at-zero', `diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -0,0 +1 @@
+first line
`);

// Large line numbers
addPatchFixture('large-line-numbers', `diff --git a/big.txt b/big.txt
--- a/big.txt
+++ b/big.txt
@@ -9999,3 +10000,4 @@
 context
-old line
+new line
+extra
 more context
`);

// File path with common prefix stripping (i/, w/, c/, o/)
addPatchFixture('prefix-i', `diff --git i/file.txt w/file.txt
--- i/file.txt
+++ w/file.txt
@@ -1 +1 @@
-old
+new
`);

// Single line file, no trailing newline
addPatchFixture('single-line-no-newline', `diff --git a/one.txt b/one.txt
--- a/one.txt
+++ b/one.txt
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
`);

// Submodule change (just metadata, no hunks)
addPatchFixture('submodule-change', `diff --git a/vendor/lib b/vendor/lib
--- a/vendor/lib
+++ b/vendor/lib
@@ -1 +1 @@
-Subproject commit abc123
+Subproject commit def456
`);

// Special chars in content
addPatchFixture('special-chars-content', `diff --git a/special.txt b/special.txt
--- a/special.txt
+++ b/special.txt
@@ -1,3 +1,3 @@
 normal
-has "quotes" and <angles> & ampersand
+has 'quotes' and {braces} | pipe
 end
`);

// Tab in content
addPatchFixture('tab-content', "diff --git a/tabs.txt b/tabs.txt\n--- a/tabs.txt\n+++ b/tabs.txt\n@@ -1,2 +1,2 @@\n \tindented with tab\n-\told content\n+\tnew content\n");

// Empty line in diff (blank context line)
addPatchFixture('blank-context-line', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,5 +1,5 @@
 first

 third
-fourth
+changed
 fifth
`);

// Three consecutive additions at start
addPatchFixture('three-adds-at-start', `diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,5 @@
+added1
+added2
+added3
 existing1
 existing2
`);

// Only context lines (no changes, but hunk exists)
addPatchFixture('only-context', `diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 line 1
 line 2
 line 3
`);

// hunk with zero-length old range
addPatchFixture('zero-old-range', `diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -5,0 +6,2 @@
+inserted a
+inserted b
`);

// No newline at end of file on deleted line
addPatchFixture('no-newline-del', `diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,2 +1,2 @@
 keep
-removed
\\ No newline at end of file
+added
\\ No newline at end of file
`);

// Diff with --no-prefix style (no a/ b/)
addPatchFixture('no-prefix', `--- file.txt
+++ file.txt
@@ -1 +1 @@
-old
+new
`);

// Rename with rename to carrying path
addPatchFixture('rename-to-path', `diff --git a/old/path.txt b/new/path.txt
similarity index 100%
rename from old/path.txt
rename to new/path.txt
`);

// Diff with git stat-only (no hunks, just diffstat)
addPatchFixture('stat-only', `diff --git a/file.txt b/file.txt
index abc123..def456 100644
`);

// Multiple mode changes and content change
addPatchFixture('mode-and-content', `diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
--- a/script.sh
+++ b/script.sh
@@ -1,2 +1,3 @@
 #!/bin/bash
 echo hello
+echo world
`);

// Deeply nested path
addPatchFixture('deep-path', `diff --git a/very/deep/nested/path/to/file.txt b/very/deep/nested/path/to/file.txt
--- a/very/deep/nested/path/to/file.txt
+++ b/very/deep/nested/path/to/file.txt
@@ -1 +1 @@
-old
+new
`);

// File with dots in name
addPatchFixture('dotted-name', `diff --git a/src/config.prod.min.js b/src/config.prod.min.js
--- a/src/config.prod.min.js
+++ b/src/config.prod.min.js
@@ -1 +1 @@
-var x=1;
+var x=2;
`);

// Multiple hunks in different files with adds and deletes
addPatchFixture('multi-file-mixed', `diff --git a/add.txt b/add.txt
new file mode 100644
--- /dev/null
+++ b/add.txt
@@ -0,0 +1 @@
+new
diff --git a/del.txt b/del.txt
deleted file mode 100644
--- a/del.txt
+++ /dev/null
@@ -1 +0,0 @@
-old
diff --git a/mod.txt b/mod.txt
--- a/mod.txt
+++ b/mod.txt
@@ -1 +1 @@
-before
+after
`);

// Backslash in path
addPatchFixture('backslash-path', `diff --git a/src/utils.ts b/src/utils.ts
--- a/src/utils.ts
+++ b/src/utils.ts
@@ -1,2 +1,2 @@
 import foo from './foo';
-export const x = 1;
+export const x = 2;
`);

// Consecutive blank context lines
addPatchFixture('consecutive-blanks', `diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,6 +1,6 @@
 a


-old
+new

 b
`);

// Large add block
addPatchFixture('large-add', (() => {
    let s = `diff --git a/big.txt b/big.txt\n--- a/big.txt\n+++ b/big.txt\n@@ -1,1 +1,21 @@\n keep\n`;
    for (let j = 0; j < 20; j++) s += `+line ${j}\n`;
    return s;
})());

// Hunk with no context lines at all
addPatchFixture('no-context-hunk', `diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,2 @@
-a
-b
+c
+d
`);

console.log(`Generated ${patchFixtures.length} patch fixtures`);
// Patches over 40 KB add size, not cases.
const smallPatchFixtures = patchFixtures.filter(fixture => JSON.stringify(fixture).length < 40000);
writeFileSync(resolve(fixturesDir, 'patch_fixtures.json'), JSON.stringify(smallPatchFixtures, null, 2));

// --- Meta fixtures ---

const metaFixtures = [];

function addMetaFixture(name, input) {
    const expected = parsePatchMeta(input);
    metaFixtures.push({ name, input, expected });
}

addMetaFixture('basic', `title: Fix authentication bug
pr: 42
repo: owner/repo
branch: fix/auth
base: main
commits: 3

This fixes the authentication bug that caused users to be logged out.`);

addMetaFixture('url-only', `url: https://github.com/owner/repo/pull/123

Body text here.`);

addMetaFixture('url-and-repo', `repo: explicit/repo
url: https://github.com/owner/repo/pull/99

Body.`);

addMetaFixture('url-and-number', `pr: 50
url: https://github.com/owner/repo/pull/99

Body.`);

addMetaFixture('empty-body', `title: Something
pr: 1
`);

addMetaFixture('no-headers', `Just a body without any headers.

Multiple paragraphs.`);

addMetaFixture('fenced', `---
title: Fenced
pr: 7
---

Body after fence.`);

addMetaFixture('fenced-with-trailing-space', `---
title: Fenced trailing
---

Body.`);

addMetaFixture('pr-zero', `pr: 0

Body.`);

addMetaFixture('commits-zero', `commits: 0

Body.`);

addMetaFixture('non-numeric-pr', `pr: abc

Body.`);

addMetaFixture('all-fields', `title: Full example
pr: 42
branch: feature/thing
base: develop
commits: 5
repo: org/project
url: https://github.com/org/project/pull/42

## Description

A full PR description with **markdown**.`);

addMetaFixture('body-only-blank', `title: X

`);

addMetaFixture('multiline-body', `title: Y

Line one.

Line two.

Line three.`);

addMetaFixture('unknown-keys', `title: Known
custom: ignored-but-parsed-past
pr: 10

Body.`);

addMetaFixture('blank-line-immediate', `
Just body.`);

addMetaFixture('url-no-github', `url: https://gitlab.com/owner/repo/merge_requests/5

Body.`);

addMetaFixture('url-with-dots', `url: https://github.com/my.org/my.repo/pull/77

Body.`);

addMetaFixture('url-with-hyphens', `url: https://github.com/my-org/my-repo/pull/88

Body.`);

addMetaFixture('title-with-colon', `title: Fix: handle edge case

Body.`);

addMetaFixture('number-key', `number: 33

Body.`);

addMetaFixture('repo-with-whitespace', `repo:   owner/repo

Body.`);

addMetaFixture('triple-dash-end', `title: A
---

Body after triple dash.`);

addMetaFixture('fenced-tab', `---\t
title: Tab fence
---

Body.`);

addMetaFixture('fenced-multi-space', `---
title: Multi space
---

Body.`);

console.log(`Generated ${metaFixtures.length} meta fixtures`);
writeFileSync(resolve(fixturesDir, 'meta_fixtures.json'), JSON.stringify(metaFixtures, null, 2));

// --- Discovery fixtures ---

const discoveryFixtures = [];

function addDiscoveryFixture(name, files, expectedDoc, expectedPatches) {
    const docMatch = (() => {
        for (const n of DOCUMENT_NAMES) {
            if (files.includes(n)) return n;
        }
        const pseudo = files.filter(f => f.endsWith('.pseudocode.md')).sort();
        return pseudo[0] || null;
    })();
    const patchMatch = files.filter(f => /\.(patch|diff)$/.test(f)).sort();
    discoveryFixtures.push({ name, files, expectedDocument: docMatch, expectedPatches: patchMatch });
}

addDiscoveryFixture('index-markdown', ['index.markdown', 'README.md', 'change.patch'], 'index.markdown', ['change.patch']);
addDiscoveryFixture('index-md', ['foo.txt', 'index.md'], 'index.md', []);
addDiscoveryFixture('INDEX-md', ['INDEX.md', 'bar.diff'], 'INDEX.md', ['bar.diff']);
addDiscoveryFixture('readme-md', ['README.md'], 'README.md', []);
addDiscoveryFixture('readme-lower', ['readme.md'], 'readme.md', []);
addDiscoveryFixture('pseudocode-fallback', ['alpha.pseudocode.md', 'beta.pseudocode.md'], 'alpha.pseudocode.md', []);
addDiscoveryFixture('no-document', ['foo.rs', 'bar.go'], null, []);
addDiscoveryFixture('patches-sorted', ['z.patch', 'a.diff', 'b.patch', 'm.diff'], null, ['a.diff', 'b.patch', 'm.diff', 'z.patch']);
addDiscoveryFixture('mixed', ['readme.md', 'change.patch', 'other.diff', 'notes.md'], 'readme.md', ['change.patch', 'other.diff']);
addDiscoveryFixture('priority-over-pseudo', ['README.md', 'design.pseudocode.md'], 'README.md', []);
addDiscoveryFixture('empty', [], null, []);

console.log(`Generated ${discoveryFixtures.length} discovery fixtures`);
writeFileSync(resolve(fixturesDir, 'discovery_fixtures.json'), JSON.stringify(discoveryFixtures, null, 2));

// --- Findings fixtures ---

const findingsFixtures = [];

findingsFixtures.push({
    name: 'json-array',
    format: 'json',
    input: JSON.stringify([
        { id: 'f1', body: 'Issue one', anchor: { type: 'code-line', path: 'src/main.rs', line: 10 }, severity: 'high', labels: ['security'] },
        { id: 'f2', body: 'Issue two', anchor: {}, verdict: 'needs fix' }
    ]),
    expected: [
        { id: 'f1', body: 'Issue one', anchor: { type: 'code-line', path: 'src/main.rs', line: 10 }, severity: 'high', labels: ['security'] },
        { id: 'f2', body: 'Issue two', anchor: {}, verdict: 'needs fix' }
    ]
});

findingsFixtures.push({
    name: 'jsonl',
    format: 'jsonl',
    input: `{"id":"f1","body":"First","anchor":{}}
{"id":"f2","body":"Second","anchor":{"type":"text"},"severity":"medium"}
`,
    expected: [
        { id: 'f1', body: 'First', anchor: {} },
        { id: 'f2', body: 'Second', anchor: { type: 'text' }, severity: 'medium' }
    ]
});

findingsFixtures.push({
    name: 'jsonl-with-blanks',
    format: 'jsonl',
    input: `

{"id":"f1","body":"Only one","anchor":{}}

`,
    expected: [
        { id: 'f1', body: 'Only one', anchor: {} }
    ]
});

findingsFixtures.push({
    name: 'json-with-proposal',
    format: 'json',
    input: JSON.stringify([
        { id: 'f1', body: 'Fix this', anchor: {}, proposal: { before: 'old text', after: 'new text', path: 'src/lib.rs', startLine: 5 } }
    ]),
    expected: [
        { id: 'f1', body: 'Fix this', anchor: {}, proposal: { before: 'old text', after: 'new text', path: 'src/lib.rs', startLine: 5 } }
    ]
});

findingsFixtures.push({
    name: 'empty-array',
    format: 'json',
    input: '[]',
    expected: []
});

findingsFixtures.push({
    name: 'empty-jsonl',
    format: 'jsonl',
    input: '',
    expected: []
});

console.log(`Generated ${findingsFixtures.length} findings fixtures`);
writeFileSync(resolve(fixturesDir, 'findings_fixtures.json'), JSON.stringify(findingsFixtures, null, 2));

const total = patchFixtures.length + metaFixtures.length + discoveryFixtures.length + findingsFixtures.length;
console.log(`\nTotal: ${total} fixtures across all categories`);
