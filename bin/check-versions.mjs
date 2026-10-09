#!/usr/bin/env node
// Fails when a file a release bumps carries a different version from extensions/review/package.json, so no agent
// integration ships behind the app (see the release steps in docs/running-and-packaging.md).
//
//   node bin/check-versions.mjs                exit 1 if any version differs (make check-versions)
//   node bin/check-versions.mjs --tag v1.2.3   also exit 1 if the release tag differs (the release workflow)
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = file => JSON.parse(readFileSync(join(root, file), 'utf8'));
const expected = read('extensions/review/package.json').version;

/** file -> the versions in it that must equal the app's: label -> value. */
const found = {
    '.claude-plugin/marketplace.json': Object.fromEntries(read('.claude-plugin/marketplace.json').plugins.map(p => [`plugin ${p.name}`, p.version])),
    'plugin/.claude-plugin/plugin.json': { version: read('plugin/.claude-plugin/plugin.json').version },
    'integrations/omp/package.json': { version: read('integrations/omp/package.json').version },
    'integrations/pi/package.json': { version: read('integrations/pi/package.json').version }
};
for (const app of ['applications/browser/package.json', 'applications/electron/package.json']) {
    const pkg = read(app);
    found[app] = { version: pkg.version, '@co-review/review': pkg.dependencies?.['@co-review/review'] };
}
const tag = process.argv.indexOf('--tag');
if (tag >= 0) {
    found['release tag'] = { tag: process.argv[tag + 1]?.replace(/^v/, '') };
}

const wrong = Object.entries(found).flatMap(([file, versions]) =>
    Object.entries(versions).filter(([, v]) => v !== expected).map(([label, v]) => `${file}: ${label} is ${v ?? 'missing'}`));
if (wrong.length) {
    console.error(`Versions differ from extensions/review/package.json (${expected}):\n  ${wrong.join('\n  ')}`);
    process.exit(1);
}
console.log(`versions: all ${expected}`);
