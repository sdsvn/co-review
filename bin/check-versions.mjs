#!/usr/bin/env node
// Fails when a file a release bumps carries a different version from the app's, Cargo.toml's [workspace.package]
// version (the single source: the app, the `co-review` command and the app's `version` file are built from it), so no
// agent integration ships behind the app (see the release steps in docs/running-and-packaging.md).
//
//   node bin/check-versions.mjs                exit 1 if any version differs (make check-versions)
//   node bin/check-versions.mjs --tag v1.2.3   also exit 1 if the release tag differs (the release workflow)
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const text = file => readFileSync(join(root, file), 'utf8');
const read = file => JSON.parse(text(file));

/** `version = "…"` in Cargo.toml's [workspace.package] table. */
function cargoVersion() {
    const table = text('Cargo.toml').split(/^\[/m).find(section => section.startsWith('workspace.package]'));
    return table?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
}
const expected = cargoVersion();
if (!expected) {
    console.error('No version in Cargo.toml\'s [workspace.package]');
    process.exit(1);
}

/** file -> the versions in it that must equal the app's: label -> value. */
const found = {
    '.claude-plugin/marketplace.json': Object.fromEntries(read('.claude-plugin/marketplace.json').plugins.map(p => [`plugin ${p.name}`, p.version])),
    'plugin/.claude-plugin/plugin.json': { version: read('plugin/.claude-plugin/plugin.json').version },
    'integrations/omp/package.json': { version: read('integrations/omp/package.json').version },
    'integrations/pi/package.json': { version: read('integrations/pi/package.json').version }
};
const tag = process.argv.indexOf('--tag');
if (tag >= 0) {
    found['release tag'] = { tag: process.argv[tag + 1]?.replace(/^v/, '') };
}

const wrong = Object.entries(found).flatMap(([file, versions]) =>
    Object.entries(versions).filter(([, v]) => v !== expected).map(([label, v]) => `${file}: ${label} is ${v ?? 'missing'}`));
if (wrong.length) {
    console.error(`Versions differ from Cargo.toml's [workspace.package] version (${expected}):\n  ${wrong.join('\n  ')}`);
    process.exit(1);
}
console.log(`versions: all ${expected}`);
