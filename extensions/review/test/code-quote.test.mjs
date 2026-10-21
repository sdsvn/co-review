// Quoted findings (src/common/code-quote.ts): a finding's `code` decides its lines, not the line number the agent wrote.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const lib = process.env.CO_REVIEW_LIB ?? '../lib';
const { locateInPatch, locateInText } = require(`${lib}/common/code-quote.js`);
const { parsePatch } = require(`${lib}/common/patch.js`);

const file = [
    'function load(id) {',
    '    const row = db.get(id);',
    '    return row;',
    '}',
    '',
    'function save(row) {',
    '    return row;',
    '}'
].join('\n');

test('a quote lands on its lines, whatever the indentation', () => {
    assert.deepEqual(locateInText(file, 'const row = db.get(id);'), { found: { start: 2, end: 2 } });
    assert.deepEqual(locateInText(file, '\n  const row = db.get(id);\n  return row;   \n'), { found: { start: 2, end: 3 } });
});

test('a quote that matches nothing is refused', () => {
    const result = locateInText(file, 'const row = db.fetch(id);', undefined, 'src/a.js');
    assert.match(result.error, /doesn't match src\/a\.js/);
    assert.match(locateInText(file, '  \n ').error, /empty/);
});

test('a quote found twice needs a line to pick one, and then takes the nearest', () => {
    assert.match(locateInText(file, 'return row;').error, /2 places .*lines 3, 7/);
    assert.deepEqual(locateInText(file, 'return row;', 6), { found: { start: 7, end: 7 } });
    assert.deepEqual(locateInText(file, 'return row;', 1), { found: { start: 3, end: 3 } });
});

const patch = parsePatch([
    'diff --git a/src/a.js b/src/a.js',
    '--- a/src/a.js',
    '+++ b/src/a.js',
    '@@ -1,4 +1,5 @@',
    ' function load(id) {',
    '-    return db.get(id);',
    '+    const row = db.get(id);',
    '+    return row;',
    ' }',
    '@@ -10,3 +11,3 @@',
    ' function save(row) {',
    '-    db.put(row);',
    '+    db.put(row, { sync: true });',
    '     return row;',
    ''
].join('\n'))[0];

test('on a patch, the new side gives new line numbers; a quote copied with diff markers matches too', () => {
    assert.deepEqual(locateInPatch(patch, 'const row = db.get(id);'), { found: { start: 2, end: 2 } });
    assert.deepEqual(locateInPatch(patch, '+    const row = db.get(id);\n+    return row;'), { found: { start: 2, end: 3 } });
    assert.deepEqual(locateInPatch(patch, 'db.put(row, { sync: true });'), { found: { start: 12, end: 12 } });
});

test('on a patch, the old side gives old line numbers, and removed lines are not on the new side', () => {
    assert.deepEqual(locateInPatch(patch, 'return db.get(id);', 'old'), { found: { start: 2, end: 2 } });
    assert.match(locateInPatch(patch, 'return db.get(id);', 'new').error, /new side of src\/a\.js/);
});

test('on a patch, a quote that touches the change wins over the same lines as context', () => {
    // `return row;` is added in the first hunk and context in the second.
    assert.deepEqual(locateInPatch(patch, 'return row;'), { found: { start: 3, end: 3 } });
});

test('on a patch, a quote does not run across two hunks', () => {
    assert.match(locateInPatch(patch, '}\nfunction save(row) {').error, /doesn't match/);
});
