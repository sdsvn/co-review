// Anchor tracking (src/common/anchor-tracking.ts): what happens to a comment's target as the file changes.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { rangeIn, track } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/common/anchor-tracking.js` : '../lib/common/anchor-tracking.js');

const file = [
    'package main',
    '',
    'func load(path string) ([]byte, error) {',
    '\treturn os.ReadFile(path)',
    '}',
    '',
    'func save(path string, data []byte) error {',
    '\treturn os.WriteFile(path, data, 0o644)',
    '}',
    ''
].join('\n');

/** A target: the given text (its first occurrence) in `text`. */
function target(text, quote, from = text.indexOf(quote)) {
    assert.ok(from >= 0, `"${quote}" is in the text`);
    return rangeIn(text, 'v1', from, from + quote.length);
}

test('lines inserted above: the target moves, unchanged (active)', () => {
    const t = target(file, '\treturn os.ReadFile(path)');
    const after = file.replace('package main\n', 'package main\n\nimport "os"\n\n// load reads a file.\n');
    const r = track(t, file, after, 'v2');
    assert.equal(r.status, 'active');
    assert.equal(after.slice(r.from, r.to), '\treturn os.ReadFile(path)');
    assert.ok(r.from > t.from);
});

test('a name changed inside the target: modified, at the same place', () => {
    const t = target(file, '\treturn os.WriteFile(path, data, 0o644)');
    const after = file.replace('os.WriteFile(path, data, 0o644)', 'os.WriteFile(name, data, 0o644)');
    const r = track(t, file, after, 'v2');
    assert.equal(r.status, 'modified');
    assert.equal(after.slice(r.from, r.to), '\treturn os.WriteFile(name, data, 0o644)');
    assert.equal(r.original, '\treturn os.WriteFile(path, data, 0o644)');
});

test('the target deleted: removed, not attached to other code', () => {
    const fn = 'func save(path string, data []byte) error {\n\treturn os.WriteFile(path, data, 0o644)\n}\n';
    const t = target(file, '\treturn os.WriteFile(path, data, 0o644)');
    const r = track(t, file, file.replace(fn, ''), 'v2');
    assert.equal(r.status, 'removed');
});

test('the function moved elsewhere in the file: found again by its text (active)', () => {
    const fn = 'func load(path string) ([]byte, error) {\n\treturn os.ReadFile(path)\n}\n\n';
    const t = target(file, '\treturn os.ReadFile(path)');
    const after = file.replace(fn, '') + '\n' + fn;
    const r = track(t, file, after, 'v2');
    assert.equal(r.status, 'active');
    assert.equal(after.slice(r.from, r.to), '\treturn os.ReadFile(path)');
});

test('the same line now in several places with the same context: ambiguous, not a guess', () => {
    const before = 'func a() error {\n\treturn nil\n}\n';
    const t = target(before, '\treturn nil');
    // The whole file is replaced (so the mapping can't place it), and three equally good copies appear.
    const after = 'func b() error {\n\treturn nil\n}\nfunc c() error {\n\treturn nil\n}\nfunc d() error {\n\treturn nil\n}\n';
    const r = track(t, before, after, 'v2', [{ from: 0, to: before.length, insert: after }]);
    assert.equal(r.status, 'ambiguous');
    assert.equal(r.candidates.length, 3);
});

test('an untouched target stays active when copies of it appear around it (whichever copy the diff aligns it with)', () => {
    const before = 'func a() error {\n\treturn nil\n}\n';
    const t = target(before, '\treturn nil');
    const after = 'func b() error {\n\treturn nil\n}\nfunc c() error {\n\treturn nil\n}\n';
    const r = track(t, before, after, 'v2');
    assert.equal(r.status, 'active');
    assert.equal(after.slice(r.from, r.to), '\treturn nil');
});

test('repeated text, but the context tells them apart: the right copy', () => {
    const before = 'x := 1\nlog.Print("a")\ny := 2\nlog.Print("a")\nz := 3\n';
    const second = before.indexOf('log.Print("a")', before.indexOf('log.Print("a")') + 1);
    const t = target(before, 'log.Print("a")', second);
    // Rewritten from scratch (no usable mapping), both copies kept, their neighbours too.
    const after = 'w := 0\n' + before;
    const r = track(t, undefined, after, 'v2');
    assert.equal(r.status, 'active');
    assert.equal(r.from, after.lastIndexOf('log.Print("a")'));
});

test('explicit edits are used as given (an edit Co-Review applied)', () => {
    const t = target(file, '\treturn os.ReadFile(path)');
    const insert = '// header\n';
    const after = insert + file;
    const r = track(t, file, after, 'v2', [{ from: 0, to: 0, insert }]);
    assert.equal(r.status, 'active');
    assert.equal(r.from, t.from + insert.length);
});

test('an insertion right at the edge of the target stays outside it', () => {
    const t = target(file, 'os.ReadFile(path)');
    const after = file.replace('os.ReadFile(path)', 'os.ReadFile(path) // read it');
    const r = track(t, file, after, 'v2');
    assert.equal(r.status, 'active');
    assert.equal(after.slice(r.from, r.to), 'os.ReadFile(path)');
});

test('unchanged file: the same target', () => {
    const t = target(file, '\treturn os.ReadFile(path)');
    const r = track(t, file, file, 'v1');
    assert.deepEqual([r.from, r.to, r.status], [t.from, t.to, 'active']);
});

test('no snapshot of the old version: placed by search alone', () => {
    const t = target(file, '\treturn os.ReadFile(path)');
    const after = '// new\n' + file;
    const r = track(t, undefined, after, 'v2');
    assert.equal(r.status, 'active');
    assert.equal(after.slice(r.from, r.to), '\treturn os.ReadFile(path)');
});
