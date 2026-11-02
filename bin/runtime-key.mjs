#!/usr/bin/env node
// Prints what the desktop app has outside its app folder (Contents/Resources/app, resources/app): the Electron it
// runs on and how electron-builder packs it around the app (the config, the icons). Two releases with the same key
// differ only inside the app folder, so install.sh updates an install of one to the other by replacing that folder
// (Co-Review-<os>-<arch>-app.tar.xz) instead of downloading the whole app. The release writes it into the app
// (runtime.txt) and next to it (Co-Review-<os>-<arch>-runtime.txt).
import { createHash } from 'crypto';
import { readdirSync, readFileSync } from 'fs';
import { join, relative } from 'path';
import { fileURLToPath } from 'url';

const root = fileURLToPath(new URL('..', import.meta.url));
const electronApp = join(root, 'applications/electron');
const electron = JSON.parse(readFileSync(join(root, 'node_modules/electron/package.json'), 'utf8')).version;

const files = dir => readdirSync(dir, { withFileTypes: true })
    .flatMap(e => e.isDirectory() ? files(join(dir, e.name)) : [join(dir, e.name)]);
const hash = createHash('sha256');
for (const file of [join(electronApp, 'electron-builder.yml'), ...files(join(electronApp, 'resources')).sort()]) {
    hash.update(relative(root, file)).update('\0').update(readFileSync(file)).update('\0');
}
process.stdout.write(`electron ${electron}, packaging ${hash.digest('hex').slice(0, 12)}\n`);
