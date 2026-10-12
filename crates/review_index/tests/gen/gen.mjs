#!/usr/bin/env node
/**
 * Generate expected symbol extraction output from the TypeScript code for fixture files.
 * Writes tests/gen/expected.json consumed by the Rust conformance test.
 *
 * Usage: node tests/gen/gen.mjs
 */

import { readFileSync, writeFileSync, readdirSync } from 'fs';
import { join, extname, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const crateRoot = join(__dirname, '..', '..');
const repoRoot = join(crateRoot, '..', '..');

// Import the compiled TypeScript syntax service.
const { SyntaxServiceImpl } = await import(join(repoRoot, 'extensions/review/lib/node/syntax-service-impl.js'));
const { LANGUAGE_BY_EXTENSION } = await import(join(repoRoot, 'extensions/review/lib/node/repo-index.js'));

const syntax = new SyntaxServiceImpl();

const fixturesDir = join(crateRoot, 'tests/fixtures');
const files = readdirSync(fixturesDir).filter(f => !f.startsWith('.'));

const results = {};

for (const file of files) {
    const ext = extname(file);
    const languageId = LANGUAGE_BY_EXTENSION[ext];
    if (!languageId) {
        console.log(`Skipping ${file}: no language mapping for ${ext}`);
        continue;
    }

    const text = readFileSync(join(fixturesDir, file), 'utf8');
    try {
        const symbols = await syntax.getSymbols(languageId, text);
        // Convert to the same shape the Rust code produces: [(name, [member_names])]
        // where members exclude fields (kind !== 'field').
        const tuples = symbols.map(s => [
            s.name,
            s.children.filter(c => c.kind !== 'field').map(c => c.name)
        ]);
        results[file] = tuples;
        console.log(`${file} (${languageId}): ${tuples.length} symbols`);
    } catch (e) {
        console.error(`Error processing ${file}:`, e.message);
        results[file] = [];
    }
}

// Also generate a render output for the fixtures directory treated as a mini-repo.
const { RepoIndex } = await import(join(repoRoot, 'extensions/review/lib/node/repo-index.js'));

const outputPath = join(__dirname, 'expected.json');
writeFileSync(outputPath, JSON.stringify(results, null, 2));
console.log(`\nWrote ${outputPath}`);
