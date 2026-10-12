#!/usr/bin/env node
/**
 * Generate test fixtures for the Rust review_anchor crate by running the TypeScript
 * anchor-tracking module. Produces cases.json with inputs and expected outputs.
 *
 * Usage: node crates/review_anchor/tests/gen/gen.mjs [repo] [commits]
 */
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

const lib = process.env.CO_REVIEW_LIB ?? resolve(__dirname, '../../../../extensions/review/lib');
const tracking = require(`${lib}/common/anchor-tracking.js`);
const { rangeIn, editsBetween, similarity, track, search, TRACKING, offsetAt, positionAt } = tracking;

const cases = [];
let caseId = 0;

function addCase(name, input, expected) {
    cases.push({ id: caseId++, name, input, expected });
}

// ─── Unit test cases from anchor-tracking.test.mjs ───

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

function target(text, quote, from) {
    if (from === undefined) from = text.indexOf(quote);
    return rangeIn(text, 'v1', from, from + quote.length);
}

// Test 1: lines inserted above
{
    const t = target(file, '\treturn os.ReadFile(path)');
    const after = file.replace('package main\n', 'package main\n\nimport "os"\n\n// load reads a file.\n');
    const r = track(t, file, after, 'v2');
    addCase('lines_inserted_above', { anchor: t, before: file, after, version: 'v2' }, r);
}

// Test 2: name changed inside target
{
    const t = target(file, '\treturn os.WriteFile(path, data, 0o644)');
    const after = file.replace('os.WriteFile(path, data, 0o644)', 'os.WriteFile(name, data, 0o644)');
    const r = track(t, file, after, 'v2');
    addCase('name_changed_inside', { anchor: t, before: file, after, version: 'v2' }, r);
}

// Test 3: target deleted
{
    const fn_ = 'func save(path string, data []byte) error {\n\treturn os.WriteFile(path, data, 0o644)\n}\n';
    const t = target(file, '\treturn os.WriteFile(path, data, 0o644)');
    const after = file.replace(fn_, '');
    const r = track(t, file, after, 'v2');
    addCase('target_deleted', { anchor: t, before: file, after, version: 'v2' }, r);
}

// Test 4: function moved
{
    const fn_ = 'func load(path string) ([]byte, error) {\n\treturn os.ReadFile(path)\n}\n\n';
    const t = target(file, '\treturn os.ReadFile(path)');
    const after = file.replace(fn_, '') + '\n' + fn_;
    const r = track(t, file, after, 'v2');
    addCase('function_moved', { anchor: t, before: file, after, version: 'v2' }, r);
}

// Test 5: ambiguous
{
    const before = 'func a() error {\n\treturn nil\n}\n';
    const t = target(before, '\treturn nil');
    const after = 'func b() error {\n\treturn nil\n}\nfunc c() error {\n\treturn nil\n}\nfunc d() error {\n\treturn nil\n}\n';
    const r = track(t, before, after, 'v2', [{ from: 0, to: before.length, insert: after }]);
    addCase('ambiguous_duplicates', { anchor: t, before, after, version: 'v2', edits: [{ from: 0, to: before.length, insert: after }] }, r);
}

// Test 6: untouched target with copies
{
    const before = 'func a() error {\n\treturn nil\n}\n';
    const t = target(before, '\treturn nil');
    const after = 'func b() error {\n\treturn nil\n}\nfunc c() error {\n\treturn nil\n}\n';
    const r = track(t, before, after, 'v2');
    addCase('untouched_with_copies', { anchor: t, before, after, version: 'v2' }, r);
}

// Test 7: repeated text, context tells apart
{
    const before = 'x := 1\nlog.Print("a")\ny := 2\nlog.Print("a")\nz := 3\n';
    const second = before.indexOf('log.Print("a")', before.indexOf('log.Print("a")') + 1);
    const t = target(before, 'log.Print("a")', second);
    const after = 'w := 0\n' + before;
    const r = track(t, undefined, after, 'v2');
    addCase('context_tells_apart', { anchor: t, before: null, after, version: 'v2' }, r);
}

// Test 8: explicit edits
{
    const t = target(file, '\treturn os.ReadFile(path)');
    const insert = '// header\n';
    const after = insert + file;
    const r = track(t, file, after, 'v2', [{ from: 0, to: 0, insert }]);
    addCase('explicit_edits', { anchor: t, before: file, after, version: 'v2', edits: [{ from: 0, to: 0, insert }] }, r);
}

// Test 9: insertion at edge
{
    const t = target(file, 'os.ReadFile(path)');
    const after = file.replace('os.ReadFile(path)', 'os.ReadFile(path) // read it');
    const r = track(t, file, after, 'v2');
    addCase('insertion_at_edge', { anchor: t, before: file, after, version: 'v2' }, r);
}

// Test 10: unchanged file
{
    const t = target(file, '\treturn os.ReadFile(path)');
    const r = track(t, file, file, 'v1');
    addCase('unchanged_file', { anchor: t, before: file, after: file, version: 'v1' }, r);
}

// Test 11: no snapshot
{
    const t = target(file, '\treturn os.ReadFile(path)');
    const after = '// new\n' + file;
    const r = track(t, undefined, after, 'v2');
    addCase('no_snapshot', { anchor: t, before: null, after, version: 'v2' }, r);
}

// ─── Edge cases: Unicode / astral characters ───

{
    const text = 'Hello 😀 world\nfoo bar\n';
    const t = target(text, '😀 world');
    const after = 'prefix\n' + text;
    const r = track(t, text, after, 'v2');
    addCase('unicode_emoji_moved', { anchor: t, before: text, after, version: 'v2' }, r);
}

{
    const text = 'abc 🎉🎊 def\nxyz\n';
    const t = target(text, '🎉🎊 def');
    const after = text.replace('🎉🎊 def', '🎉🎊 ghi');
    const r = track(t, text, after, 'v2');
    addCase('unicode_emoji_modified', { anchor: t, before: text, after, version: 'v2' }, r);
}

{
    const text = '日本語テスト\nHello world\n';
    const t = target(text, '日本語テスト');
    const after = 'added line\n' + text;
    const r = track(t, text, after, 'v2');
    addCase('unicode_cjk', { anchor: t, before: text, after, version: 'v2' }, r);
}

// ─── Edge cases: empty/whitespace target ───

{
    const text = 'hello world\n';
    const t = rangeIn(text, 'v1', 5, 6); // a space
    const after = 'hello world\n';
    const r = track(t, text, after, 'v2');
    addCase('whitespace_target', { anchor: t, before: text, after, version: 'v2' }, r);
}

{
    const text = 'line one\nline two\n';
    const t = rangeIn(text, 'v1', 0, 0); // empty range
    const r = track(t, text, text, 'v2');
    addCase('empty_range', { anchor: t, before: text, after: text, version: 'v2' }, r);
}

// ─── Edge cases: moved block ───

{
    const before = 'func alpha() {\n\tprint("alpha")\n}\n\nfunc beta() {\n\tprint("beta")\n}\n\nfunc gamma() {\n\tprint("gamma")\n}\n';
    const t = target(before, 'func alpha() {\n\tprint("alpha")\n}');
    const after = 'func beta() {\n\tprint("beta")\n}\n\nfunc gamma() {\n\tprint("gamma")\n}\n\nfunc alpha() {\n\tprint("alpha")\n}\n';
    const r = track(t, before, after, 'v2');
    addCase('block_moved_to_end', { anchor: t, before, after, version: 'v2' }, r);
}

// ─── Edge cases: reworded ───

{
    const before = 'func process(data []byte) error {\n\tresult := transform(data)\n\treturn save(result)\n}\n';
    const t = target(before, '\tresult := transform(data)');
    const after = 'func process(data []byte) error {\n\tresult := transformData(data)\n\treturn save(result)\n}\n';
    const r = track(t, before, after, 'v2');
    addCase('reworded_line', { anchor: t, before, after, version: 'v2' }, r);
}

// ─── Similarity function tests ───

{
    const simCases = [
        { a: 'hello', b: 'hello', name: 'identical' },
        { a: '', b: '', name: 'both_empty' },
        { a: 'hello', b: 'world', name: 'different' },
        { a: 'hello world', b: 'hello werld', name: 'similar' },
        { a: 'abcdef', b: 'abcxyz', name: 'half_match' },
        { a: '😀hello', b: '😀hello', name: 'emoji_identical' },
    ];
    for (const { a, b, name } of simCases) {
        addCase(`similarity_${name}`, { type: 'similarity', a, b }, { similarity: similarity(a, b) });
    }
}

// ─── offsetAt / positionAt tests ───

{
    const text = 'abc\ndef\nghi';
    const offsetCases = [
        { line: 0, character: 0, name: 'start' },
        { line: 0, character: 2, name: 'mid_first_line' },
        { line: 1, character: 0, name: 'second_line' },
        { line: 1, character: 2, name: 'mid_second_line' },
        { line: 2, character: 1, name: 'third_line' },
        { line: 5, character: 0, name: 'past_end' },
    ];
    for (const { line, character, name } of offsetCases) {
        const offset = offsetAt(text, line, character);
        const pos = positionAt(text, offset);
        addCase(`offset_position_${name}`, { type: 'offset', text, line, character }, { offset, position: pos });
    }
}

// ─── editsBetween tests ───

{
    const editCases = [
        { before: 'hello world', after: 'hello world', name: 'identical' },
        { before: 'hello', after: 'hello world', name: 'append' },
        { before: 'hello world', after: 'hello', name: 'truncate' },
        { before: 'line1\nline2\nline3\n', after: 'line1\nmodified\nline3\n', name: 'modify_middle' },
    ];
    for (const { before, after, name } of editCases) {
        const edits = editsBetween(before, after);
        addCase(`edits_${name}`, { type: 'edits', before, after }, { edits });
    }
}

// ─── Git corpus: mine real history ───

const repo = process.argv[2] ?? resolve(__dirname, '../../../..');
const commits = Number(process.argv[3] ?? 80);

const git = (...args) => {
    try {
        return execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
    } catch {
        return '';
    }
};

console.log(`Mining ${commits} commits from ${repo}...`);

const log = git('log', `--no-merges`, `-${commits}`, '--format=%H %P').trim().split('\n')
    .map(l => l.split(' ')).filter(p => p.length === 2);

let corpusCount = 0;
// Budget: keep the fixture under ~3 MB.  Prefer small files and at most one case per file
// to stay diverse.  Non-active statuses (modified/removed/ambiguous) are rare and always kept.
const maxCorpusSizeBytes = 2_800_000; // leave room for non-corpus cases
const maxCorpusPerFile = 2; // at most 2 lines per file, to diversify
const maxFileLen = 50_000; // skip large files to keep case size manageable

const corpusCandidates = []; // { name, input, expected, size }

for (const [commit, parent] of log) {
    const filesRaw = git('diff', '--name-only', '--diff-filter=M', parent, commit);
    const files = filesRaw.split('\n').filter(f => /\.(ts|tsx|js|mjs|go|py|md|rs)$/.test(f));

    for (const fname of files) {
        let before, after;
        try {
            before = git('show', `${parent}:${fname}`);
            after = git('show', `${commit}:${fname}`);
        } catch {
            continue;
        }
        if (!before || !after || before.length > maxFileLen) continue;

        const edits = editsBetween(before, after);
        const lines = before.split('\n');
        let offset = 0;
        let fileCases = 0;
        lines.forEach((text, i) => {
            const trimmed = text.trim();
            const from = offset + text.length - text.trimStart().length;
            offset += text.length + 1;
            if (i % 6 === 0 && trimmed.length >= 12 && fileCases < maxCorpusPerFile) {
                const to = from + trimmed.length;
                const anchor = rangeIn(before, 'v1', from, to);
                const result = track(anchor, before, after, 'v2', edits);
                const entry = {
                    name: `corpus_${fname.replace(/\//g, '_')}_L${i + 1}`,
                    input: { anchor, before, after, version: 'v2', edits },
                    expected: result,
                };
                entry.size = JSON.stringify(entry).length;
                corpusCandidates.push(entry);
                fileCases++;
            }
        });
    }
}

// Sort: non-active first (always kept), then by size ascending (prefer small)
corpusCandidates.sort((a, b) => {
    const aNonActive = a.expected.status !== 'active' ? 0 : 1;
    const bNonActive = b.expected.status !== 'active' ? 0 : 1;
    if (aNonActive !== bNonActive) return aNonActive - bNonActive;
    return a.size - b.size;
});

let corpusBudget = maxCorpusSizeBytes;
for (const entry of corpusCandidates) {
    if (entry.size > corpusBudget && entry.expected.status === 'active') continue;
    corpusBudget -= entry.size;
    addCase(entry.name, entry.input, entry.expected);
    corpusCount++;
    if (corpusBudget <= 0) break;
}

console.log(`Generated ${cases.length} cases (${corpusCount} from git corpus)`);

const outPath = resolve(__dirname, '../fixtures/cases.json');
writeFileSync(outPath, JSON.stringify(cases, null, 2));
const outSize = JSON.stringify(cases, null, 2).length;
console.log(`Written to ${outPath} (${(outSize / 1024 / 1024).toFixed(1)} MB)`);
