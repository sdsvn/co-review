// The debug log (src/node/debug-log.ts): a loop can't flood it, and what it drops is said.
// Run after `tsc` (lib/): `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const require = createRequire(import.meta.url);
const { DebugLog, watchEventLoop } = require(process.env.CO_REVIEW_LIB ? `${process.env.CO_REVIEW_LIB}/node/debug-log.js` : '../lib/node/debug-log.js');

/** A log in a home of its own; `lines()` is what it wrote, without time stamps and process ids. */
function fresh(name) {
    const home = mkdtempSync(join(tmpdir(), 'co-review-log-'));
    process.env.CO_REVIEW_HOME = home;
    const log = new DebugLog(name);
    return {
        log,
        lines: () => readFileSync(join(home, 'logs', `${name}.log`), 'utf8').trimEnd().split('\n').map(l => l.replace(/^\S+ \d+ /, '')),
        done: () => rmSync(home, { recursive: true, force: true })
    };
}

test('an entry repeated with other numbers (an id) is counted, and the latest of them told', t => {
    const { log, lines, done } = fresh('a');
    t.after(done);
    for (let i = 0; i < 5000; i++) {
        log.warn(`No reply handler for error reply with id: ${i}`);
    }
    log.warn('event loop blocked for 7525 ms');
    log.warn('event loop blocked for 438851 ms');
    log.error('same error');
    log.error('same error');
    log.info('next');
    assert.deepEqual(lines(), [
        'WARN  No reply handler for error reply with id: 0',
        'WARN  (the entry above came 4999 more times, the last: No reply handler for error reply with id: 4999)',
        'WARN  event loop blocked for 7525 ms',
        'WARN  (the entry above came 1 more time, the last: event loop blocked for 438851 ms)',
        'ERROR same error',
        'ERROR (the entry above came 1 more time)',
        'INFO  next'
    ]);
});

test('at most 50 different entries a second; the rest are dropped and counted, within the second', async t => {
    const { log, lines, done } = fresh('b');
    t.after(done);
    for (let i = 0; i < 120; i++) {
        log.error(`kind ${String.fromCharCode(65 + i % 26).repeat(1 + Math.floor(i / 26))}`);
    }
    assert.equal(lines().length, 50);
    await new Promise(r => setTimeout(r, 1100));
    assert.equal(lines().at(-1), 'WARN  (70 more entries dropped in a second: too many)');
    log.info('a second later');
    assert.equal(lines().at(-1), 'INFO  a second later');
});

test('an entry is cut at 4 KB', t => {
    const { log, lines, done } = fresh('c');
    t.after(done);
    log.info('x'.repeat(10_000));
    assert.match(lines()[0], /^INFO  x{4096} … \(5904 more characters\)$/);
});

test('a short stall is a blocked event loop; a long gap is a pause (the computer slept)', t => {
    t.mock.timers.enable({ apis: ['setInterval', 'Date'] });
    const written = [];
    watchEventLoop({ warn: text => written.push(`warn ${text}`), info: text => written.push(`info ${text}`) }, 1000);
    t.mock.timers.tick(500);
    assert.deepEqual(written, []);
    t.mock.timers.setTime(Date.now() + 2000); // the next tick comes 2 s late
    t.mock.timers.tick(500);
    assert.deepEqual(written, ['warn event loop blocked for 2000 ms']);
    t.mock.timers.setTime(Date.now() + 3_600_000);
    t.mock.timers.tick(500);
    assert.equal(written.at(-1), 'info paused for 3600 s: the computer slept, or the event loop was blocked that long');
});
