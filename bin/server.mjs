// Finds a running Co-Review (desktop or browser) or starts the browser app in the background.
import { spawn } from 'node:child_process';
import { appendFileSync, existsSync, mkdirSync, openSync, readFileSync, renameSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, '..', 'applications', 'browser');
const pluginsDir = resolve(here, '..', 'plugins');
// Inside the desktop app (Co-Review.app/…/app/bin) there is no browser app: the CLI runs on the app's
// own Electron (ELECTRON_RUN_AS_NODE) and starts the desktop app instead.
const desktopApp = !existsSync(appDir) && !!process.versions.electron ? process.execPath : undefined;
export const home = process.env.CO_REVIEW_HOME || join(homedir(), '.co-review');
export const logsDir = join(home, 'logs');
// stdout may belong to a protocol (MCP); log to stderr, and to logs/cli.log (see `co-review logs`).
export const log = (...m) => {
    console.error('[co-review]', ...m);
    try {
        mkdirSync(logsDir, { recursive: true });
        const text = m.map(x => (x instanceof Error ? x.stack ?? x.message : String(x))).join(' ');
        appendFileSync(join(logsDir, 'cli.log'), `${new Date().toISOString()} ${process.pid} INFO  ${process.argv[2] ?? 'start'}: ${text}\n`);
    } catch {
        /* no log, rather than no command */
    }
};

async function isRunning(url) {
    try {
        const res = await fetch(`${url}/mcp`, { method: 'GET', signal: AbortSignal.timeout(1500) });
        return res.status === 400 || res.status === 404 || res.ok;
    } catch {
        return false;
    }
}

/** The base URL of a Co-Review server that is already running, or undefined; never starts one. */
export async function findServer({ port }) {
    if (!port) {
        const url = registered();
        if (url && await isRunning(url)) {
            return url;
        }
    }
    if (desktopApp) {
        return undefined;
    }
    const base = `http://127.0.0.1:${port || 3000}`;
    return await isRunning(base) ? base : undefined;
}

/**
 * Returns the base URL of a Co-Review server. Without an explicit port a running instance
 * (recorded in ~/.co-review/server.json) is reused; otherwise the browser app is started.
 */
export async function ensureServer({ port, root, open }) {
    if (desktopApp) {
        // An app of another version, or one left without a backend, is not reused: stopped, and started afresh.
        stopStaleDesktop();
    }
    if (desktopApp && open) {
        return startDesktop(root);
    }
    const running = await findServer({ port });
    if (running) {
        return running;
    }
    if (desktopApp) {
        // For an agent: in the background, without a window; the window opens when a review is shown.
        return startDesktop(undefined);
    }
    const base = `http://127.0.0.1:${port || 3000}`;
    mkdirSync(home, { recursive: true });
    // The server's output, kept from growing without bound: moved aside once it passes 5 MB.
    const logFile = join(home, 'server.log');
    try {
        if (statSync(logFile).size > 5 * 1024 * 1024) {
            renameSync(logFile, join(home, 'server.1.log'));
        }
    } catch {
        /* no log yet */
    }
    const out = openSync(logFile, 'a');
    log(`starting Co-Review on ${base} (log: ${join(home, 'server.log')})`);
    const child = spawn(process.execPath, [join(appDir, 'lib', 'backend', 'main.js'),
        '--hostname', '127.0.0.1', '--port', String(port || 3000), `--plugins=local-dir:${pluginsDir}`, root], {
        cwd: appDir, detached: true, stdio: ['ignore', out, out], env: process.env
    });
    child.unref();
    for (let i = 0; i < 120; i++) {
        if (await isRunning(base)) {
            return base;
        }
        await new Promise(r => setTimeout(r, 500));
    }
    throw new Error(`Co-Review did not start on ${base}; see the server log.`);
}

function registered() {
    try {
        return JSON.parse(readFileSync(join(home, 'server.json'), 'utf8')).url;
    } catch {
        return undefined;
    }
}

/**
 * Opens `root` in the desktop app (a running instance picks it up), or with no `root` starts it in the background
 * (no window), and waits for its backend to register.
 */
async function startDesktop(root) {
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    log(root ? `starting the Co-Review desktop app for ${root}` : 'starting the Co-Review desktop app in the background');
    spawn(desktopApp, root ? [root] : ['--background'], { detached: true, stdio: 'ignore', env }).unref();
    for (let i = 0; i < 120; i++) {
        const url = registered();
        if (url && await isRunning(url)) {
            return url;
        }
        await new Promise(r => setTimeout(r, 500));
    }
    throw new Error('The Co-Review desktop app did not start.');
}

/** The version of the Co-Review this command is part of (the app's package.json), or undefined in a checkout. */
function ownVersion() {
    try {
        return JSON.parse(readFileSync(resolve(here, '..', 'package.json'), 'utf8')).version;
    } catch {
        return undefined;
    }
}

/**
 * Stops a running desktop app that must not be used: one of another version than this command's (an update was
 * installed while it ran; the old one would keep serving, with the new one's agents and command), or one that runs
 * without a backend (its files were replaced under it; macOS kills the helper processes, not the app). Either holds
 * the single-instance lock, so a launch would be handed to it and nothing would start. The app records its process
 * and version in logs/app.json, the backend its process in server.json. Returns whether one was stopped. Exported
 * for its test; `version` stands in for this command's own.
 */
export function stopStaleDesktop({ now = Date.now(), graceMs = 90_000, version = ownVersion() } = {}) {
    const read = file => {
        try {
            return JSON.parse(readFileSync(file, 'utf8'));
        } catch {
            return undefined;
        }
    };
    const alive = pid => {
        try {
            process.kill(pid, 0);
            return true;
        } catch (e) {
            return e.code === 'EPERM';
        }
    };
    const app = read(join(logsDir, 'app.json'));
    if (!app?.pid || !alive(app.pid)) {
        return false;
    }
    let why;
    if (version && app.version && app.version !== version) {
        why = `it is version ${app.version}, and ${version} is installed`;
    } else if (now - Date.parse(app.started ?? 0) < graceMs) {
        // Still starting (its backend registers within seconds): leave it be.
        return false;
    } else {
        const server = read(join(home, 'server.json'));
        if (server?.pid && alive(server.pid)) {
            return false;
        }
        why = 'it runs without a backend, so nothing could start';
    }
    log(`stopping Co-Review (process ${app.pid}): ${why}`);
    try {
        process.kill(app.pid, 'SIGTERM');
    } catch {
        return false;
    }
    // Ten seconds for it to go; then without ceremony, as a start waits on this.
    const until = Date.now() + 10_000;
    while (alive(app.pid) && Date.now() < until) {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
    }
    if (alive(app.pid)) {
        try {
            process.kill(app.pid, 'SIGKILL');
        } catch {
            /* gone meanwhile */
        }
    }
    return true;
}

export function openUrl(url) {
    spawn(process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'explorer' : 'xdg-open', [url], { stdio: 'ignore', detached: true }).unref();
}
