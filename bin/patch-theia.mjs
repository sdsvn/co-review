#!/usr/bin/env node
// Fixes to Theia that Co-Review needs before a release of Theia has them, applied to node_modules after every
// install (npm's postinstall), before the app is bundled. Each patch is idempotent and says what it fixes; a patch
// whose target changed (a Theia upgrade) fails loudly, so it is looked at again rather than silently skipped.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const PATCHES = [
    {
        // An extension host's log message to a window that has gone (closed, reloading) is rejected; the rejection
        // was never handled, so Theia's unhandled-rejection handler logged it, twice (message and stack), through the
        // same logger, which was rejected again: every failure became two, every second. A window stuck like that
        // keeps a CPU busy and logs millions of errors. A log that can't be delivered is dropped instead.
        file: 'node_modules/@theia/plugin-ext/lib/plugin/logger.js',
        from: 'this.logger.$log(level, this.name, this.toLog(message), params.map(e => this.toLog(e)));',
        to: 'this.logger.$log(level, this.name, this.toLog(message), params.map(e => this.toLog(e)))?.catch?.(() => undefined); /* co-review: see bin/patch-theia.mjs */'
    }
];

let failed = false;
for (const patch of PATCHES) {
    const file = join(root, patch.file);
    let text;
    try {
        text = readFileSync(file, 'utf8');
    } catch {
        console.error(`patch-theia: ${patch.file} is missing`);
        failed = true;
        continue;
    }
    if (text.includes(patch.to)) {
        continue;
    }
    if (!text.includes(patch.from)) {
        console.error(`patch-theia: ${patch.file} changed; update the patch in bin/patch-theia.mjs`);
        failed = true;
        continue;
    }
    writeFileSync(file, text.replace(patch.from, patch.to));
    console.log(`patch-theia: patched ${patch.file}`);
}
process.exitCode = failed ? 1 : 0;
