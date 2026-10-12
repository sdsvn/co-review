#!/usr/bin/env node
/**
 * Extract symbols from the repository using the TypeScript code and output as JSON.
 * Used for comparing against the Rust implementation on the full repository.
 *
 * Usage: node tests/gen/repo_compare.mjs > /tmp/ts_symbols.json
 */

import { readFileSync, writeFileSync, statSync } from 'fs';
import { join, extname, dirname } from 'path';
import { fileURLToPath } from 'url';
import { execSync } from 'child_process';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, '..', '..', '..', '..');

const { SyntaxServiceImpl } = await import(join(repoRoot, 'extensions/review/lib/node/syntax-service-impl.js'));
const { LANGUAGE_BY_EXTENSION } = await import(join(repoRoot, 'extensions/review/lib/node/repo-index.js'));

const syntax = new SyntaxServiceImpl();

// Get source files from git.
const stdout = execSync('git ls-files -z --cached --others --exclude-standard', {
    cwd: repoRoot,
    maxBuffer: 64 * 1024 * 1024,
}).toString();

const allFiles = stdout.split('\0').filter(Boolean);
const sourceFiles = [...new Set(allFiles)]
    .filter(f => LANGUAGE_BY_EXTENSION[extname(f)])
    .sort()
    .slice(0, 5000);

const MAX_FILE_BYTES = 256 * 1024;
const results = {};

for (const file of sourceFiles) {
    const absolute = join(repoRoot, file);
    try {
        const stat = statSync(absolute);
        if (!stat.isFile() || stat.size > MAX_FILE_BYTES) continue;

        const text = readFileSync(absolute, 'utf8');
        const languageId = LANGUAGE_BY_EXTENSION[extname(file)];
        const symbols = await syntax.getSymbols(languageId, text);
        const tuples = symbols.map(s => [
            s.name,
            s.children.filter(c => c.kind !== 'field').map(c => c.name)
        ]);
        results[file] = tuples;
    } catch {
        // Skip files that can't be read.
    }
}

const output = JSON.stringify(results, null, 2);
process.stdout.write(output);
process.stderr.write(`\nProcessed ${Object.keys(results).length} files\n`);
