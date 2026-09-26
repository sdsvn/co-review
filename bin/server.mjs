// Finds a running Co-Review (desktop or browser) or starts the browser app in the background.
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, openSync, readFileSync } from 'node:fs';
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
// stdout may belong to a protocol (MCP); log to stderr.
export const log = (...m) => console.error('[co-review]', ...m);

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
    if (desktopApp && open) {
        return startDesktop(root);
    }
    const running = await findServer({ port });
    if (running) {
        return running;
    }
    if (desktopApp) {
        return startDesktop(root);
    }
    const base = `http://127.0.0.1:${port || 3000}`;
    mkdirSync(home, { recursive: true });
    const out = openSync(join(home, 'server.log'), 'a');
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

/** Opens `root` in the desktop app (a running instance picks it up) and waits for its backend to register. */
async function startDesktop(root) {
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    log(`starting the Co-Review desktop app for ${root}`);
    spawn(desktopApp, [root], { detached: true, stdio: 'ignore', env }).unref();
    for (let i = 0; i < 120; i++) {
        const url = registered();
        if (url && await isRunning(url)) {
            return url;
        }
        await new Promise(r => setTimeout(r, 500));
    }
    throw new Error('The Co-Review desktop app did not start.');
}

export function openUrl(url) {
    spawn(process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'explorer' : 'xdg-open', [url], { stdio: 'ignore', detached: true }).unref();
}
