// The `co-review` command (bin/server.mjs): a desktop app that must not be used is stopped before one is started.
// Run: `node --test extensions/review/test` (make test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const alive = pid => {
    try {
        process.kill(pid, 0);
        return true;
    } catch (e) {
        return e.code === 'EPERM';
    }
};
const gone = async pid => {
    for (let i = 0; i < 50 && alive(pid); i++) {
        await new Promise(r => setTimeout(r, 100));
    }
    return !alive(pid);
};

/** A process that is nobody's child here (like a real app): a stopped one is gone, not a zombie waiting to be reaped. */
const sleeper = () => Number(spawnSync('sh', ['-c', 'sleep 300 >/dev/null 2>&1 & echo $!'], { encoding: 'utf8' }).stdout.trim());

/** A home with an "app" (a sleeping process) started `started`, of `version`, and optionally a live "backend". */
async function home(t, { version = '0.6.0', started = '2026-01-01T00:00:00.000Z', backend = false } = {}) {
    const dir = mkdtempSync(join(tmpdir(), 'co-review-home-'));
    mkdirSync(join(dir, 'logs'));
    const app = { pid: sleeper() };
    const server = backend ? { pid: sleeper() } : undefined;
    t.after(() => {
        for (const p of [app, server]) {
            try {
                p && process.kill(p.pid, 'SIGKILL');
            } catch {
                /* already gone */
            }
        }
        rmSync(dir, { recursive: true, force: true });
    });
    writeFileSync(join(dir, 'logs', 'app.json'), JSON.stringify({ pid: app.pid, version, started }));
    if (server) {
        writeFileSync(join(dir, 'server.json'), JSON.stringify({ url: 'http://127.0.0.1:1', pid: server.pid }));
    }
    process.env.CO_REVIEW_HOME = dir;
    const { stopStaleDesktop } = await import(`../../../bin/server.mjs?home=${encodeURIComponent(dir)}`);
    return { app, stop: options => stopStaleDesktop({ version: '0.6.1', ...options }) };
}

test('an app of the installed version with a live backend is left alone', async t => {
    const { app, stop } = await home(t, { version: '0.6.1', backend: true });
    assert.equal(stop(), false);
    assert.ok(alive(app.pid));
});

test('an app still starting (no backend yet) is left alone', async t => {
    const { app, stop } = await home(t, { version: '0.6.1', started: new Date().toISOString() });
    assert.equal(stop(), false);
    assert.ok(alive(app.pid));
});

test('an app running without a backend is stopped', async t => {
    const { app, stop } = await home(t, { version: '0.6.1' });
    assert.equal(stop(), true);
    assert.ok(await gone(app.pid));
});

test('an app of another version than the installed one is stopped, even with its backend up', async t => {
    const { app, stop } = await home(t, { version: '0.6.0', backend: true, started: new Date().toISOString() });
    assert.equal(stop(), true);
    assert.ok(await gone(app.pid));
});

test('in a checkout (no version of its own) only the backend counts', async t => {
    const { app, stop } = await home(t, { version: '0.6.0', backend: true });
    assert.equal(stop({ version: undefined }), false);
    assert.ok(alive(app.pid));
});
