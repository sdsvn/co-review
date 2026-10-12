#!/usr/bin/env node
/**
 * Records the reference behaviour of the TypeScript BundleService (extensions/review/lib/node/bundle-service.js)
 * for the scripted review-directory scenarios in tests/fixtures/service/script.json.
 *
 * Run from the repository root (the compiled extension and its node_modules must exist):
 *   NODE_PATH=<co-review checkout>/node_modules node crates/review_bundle/tests/gen/gen_service.mjs
 *
 * Output: crates/review_bundle/tests/fixtures/service/expected.json
 * Ids, times, clocks and the temporary paths are normalized the same way the Rust test normalizes its own run.
 */
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync, appendFileSync } from 'fs';
import { createRequire } from 'module';
import * as os from 'os';
import { dirname, join, relative, resolve } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const fixtures = resolve(__dirname, '../fixtures/service');
const lib = resolve(__dirname, '../../../../extensions/review/lib');

const root = realpathSync(mkdtempSync(join(os.tmpdir(), 'bundle-service-')));
const home = join(root, 'home');
mkdirSync(home, { recursive: true });
process.env.CO_REVIEW_HOME = home;

require('reflect-metadata');
const { BundleService } = require(join(lib, 'node/bundle-service.js'));
const { ReviewStore } = require(join(lib, 'node/review-store.js'));
const { AnchorTracker } = require(join(lib, 'node/anchor-tracker.js'));
const { acceptedSuggestions } = require(join(lib, 'node/review-payloads.js'));

const author = { id: 'agent:test', kind: 'agent', name: 'test-agent' };
const script = JSON.parse(readFileSync(join(fixtures, 'script.json'), 'utf8'));

function filesOf(dir, skip) {
    const out = {};
    const walk = d => {
        for (const name of readdirSync(d).sort()) {
            const file = join(d, name);
            if (statSync(file).isDirectory()) {
                walk(file);
            } else if (!skip.includes(file)) {
                out[relative(dir, file)] = readFileSync(file, 'utf8');
            }
        }
    };
    walk(dir);
    return out;
}

const results = {};
for (const scenario of script) {
    const store = new ReviewStore();
    const tracker = new AnchorTracker();
    tracker.store = store;
    const service = new BundleService();
    service.store = store;
    service.tracker = tracker;

    const caseDir = join(root, 'cases', scenario.name);
    if (scenario.input) {
        cpSync(join(fixtures, scenario.input), caseDir, { recursive: true });
    }
    let reviewId;
    let dir;
    const record = { opened: [], synced: [], snapshots: [] };
    const threadOf = async sourceId => (await store.get(reviewId)).threads.find(t => t.sourceId === sourceId);

    for (const step of scenario.steps) {
        switch (step.op) {
            case 'open': {
                const opened = await service.open({
                    dir: scenario.input ? caseDir : undefined,
                    markdown: step.markdown,
                    patch: step.patch,
                    patchName: step.patchName,
                    title: step.title,
                    storePath: step.storePath ? join(caseDir, step.storePath) : undefined,
                    openspec: step.openspec ? join(caseDir, step.openspec) : undefined,
                    author
                });
                // The scenario drives syncing itself; a live watcher would race with it.
                for (const watcher of service.watchers.values()) {
                    watcher.close();
                }
                service.watchers.clear();
                reviewId = opened.review.id;
                dir = opened.dir;
                record.opened.push({ dir: opened.dir, docFile: opened.docFile ?? null, patches: opened.patches, threads: opened.review.threads.length });
                break;
            }
            case 'addFinding':
                await service.addFinding(reviewId, dir, service.documentOf(dir), step.finding);
                break;
            case 'decide':
                await service.decideProposal(reviewId, (await threadOf(step.sourceId)).id, step.accept);
                break;
            case 'sync':
                record.synced.push(await service.syncFindings(reviewId));
                break;
            case 'append':
                appendFileSync(join(dir, step.file), step.text);
                break;
            case 'export':
                await service.export(reviewId);
                break;
            case 'snapshot': {
                const review = await store.get(reviewId);
                const stateFile = review.bundle.storePath ?? join(dir, 'review.json');
                record.snapshots.push({
                    label: step.label,
                    review,
                    stateFile: existsSync(stateFile) ? JSON.parse(readFileSync(stateFile, 'utf8')) : null,
                    files: filesOf(dir, [stateFile]),
                    acceptedSuggestions: acceptedSuggestions(review)
                });
                break;
            }
            default:
                throw new Error(`unknown op ${step.op}`);
        }
    }
    results[scenario.name] = record;
}

// --- normalization (mirrored in tests/service.rs) ---

const TIME_KEYS = new Set(['createdAt', 'updatedAt', 'at', 'archivedAt', 'postedAt']);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const CLOCK = /^\d\d:\d\d$/;

function normalize(value) {
    const ids = new Map();
    const visit = (v, key) => {
        if (Array.isArray(v)) {
            return v.map(x => visit(x));
        }
        if (v && typeof v === 'object') {
            const out = {};
            for (const k of Object.keys(v).sort()) {
                if (v[k] !== undefined) {
                    out[k] = visit(v[k], k);
                }
            }
            return out;
        }
        if (typeof v === 'number' && (TIME_KEYS.has(key) || key === 'submittedAt')) {
            return 0;
        }
        if (typeof v === 'string') {
            if (UUID.test(v)) {
                if (!ids.has(v)) {
                    ids.set(v, `<ID${ids.size}>`);
                }
                return ids.get(v);
            }
            if ((key === 'time' || key === 'submittedAt') && CLOCK.test(v)) {
                return '<CLOCK>';
            }
            return v.split(root).join('<ROOT>');
        }
        return v;
    };
    return visit(value);
}

const out = {};
for (const [name, record] of Object.entries(results)) {
    out[name] = normalize(record);
}
writeFileSync(join(fixtures, 'expected.json'), JSON.stringify(out, undefined, 1) + '\n');
rmSync(root, { recursive: true, force: true });
console.log(`wrote ${join(fixtures, 'expected.json')}`);
process.exit(0);
