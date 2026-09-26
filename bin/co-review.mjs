#!/usr/bin/env node
// Co-Review command line.
//
//   co-review [dir]        start Co-Review (or reuse a running one) and open dir in the browser
//   co-review mcp [dir]    MCP server on stdio for agent harnesses; starts Co-Review on demand
//                          and bridges to its /mcp endpoint with dir (default: cwd) as repository
//   co-review status [dir] open reviews of dir (default: cwd) in a running Co-Review; never starts it.
//                          --claude-hook prints them as Claude Code SessionStart context (or nothing)
//   co-review setup <pi|omp>
//                          install the Pi or Oh My Pi package that ships with this Co-Review
//                          (runs `pi install` / `omp install` with its path; no clone needed)
//
// Options: --port <n> (default: reuse a running instance, else 3000; or $CO_REVIEW_PORT), --no-open
import { dirname, join, resolve } from 'node:path';
import { existsSync, readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { ensureServer, home, log, openUrl } from './server.mjs';

const args = process.argv.slice(2);
const take = name => {
    const i = args.indexOf(name);
    return i < 0 ? undefined : args.splice(i, 2)[1];
};
const port = Number(take('--port') ?? process.env.CO_REVIEW_PORT ?? 0) || undefined;
const noOpen = args.includes('--no-open') && !!args.splice(args.indexOf('--no-open'), 1);
const claudeHook = args.includes('--claude-hook') && !!args.splice(args.indexOf('--claude-hook'), 1);
const command = ['mcp', 'status', 'setup'].includes(args[0]) ? args.shift() : 'start';
const root = resolve(args[0] ?? process.cwd());

async function start() {
    const base = await ensureServer({ port, root, open: !noOpen });
    const url = `${base}/#${root}`;
    log(url);
    // The desktop app opens the repository itself.
    if (!noOpen && !process.versions.electron) {
        openUrl(url);
    }
}

async function mcp() {
    const { StdioServerTransport } = await import('@modelcontextprotocol/sdk/server/stdio.js');
    const { StreamableHTTPClientTransport } = await import('@modelcontextprotocol/sdk/client/streamableHttp.js');
    const base = await ensureServer({ port, root });
    const stdio = new StdioServerTransport();
    const http = new StreamableHTTPClientTransport(new URL(`${base}/mcp?root=${encodeURIComponent(root)}`));
    // Forward JSON-RPC messages both ways; the HTTP transport handles the MCP session id.
    stdio.onmessage = message => http.send(message).catch(e => log('forward failed:', e.message));
    http.onmessage = message => stdio.send(message);
    http.onerror = e => log('http:', e.message);
    stdio.onclose = () => http.close();
    http.onclose = () => process.exit(0);
    await http.start();
    await stdio.start();
}

/** Open reviews of `root`, from a running Co-Review only: never starts it (for hooks and scripts). */
async function status() {
    try {
        const { url } = JSON.parse(readFileSync(join(home, 'server.json'), 'utf8'));
        const res = await fetch(`${url}/api/m/status?root=${encodeURIComponent(root)}${claudeHook ? '&format=claude-hook' : ''}`, { signal: AbortSignal.timeout(1500) });
        if (res.status === 200) {
            console.log(claudeHook ? await res.text() : JSON.stringify(await res.json(), undefined, 2));
        }
    } catch {
        /* Co-Review isn't running */
    }
}

/** Agent packages shipped next to this CLI (in the desktop app and in a checkout): harness -> package dir. */
const PACKAGES = { pi: 'pi', omp: 'omp' };

/** Installs a bundled agent package with the harness's own installer. */
async function setup() {
    const harness = args[0];
    if (!PACKAGES[harness]) {
        throw new Error(`usage: co-review setup <${Object.keys(PACKAGES).join('|')}>`);
    }
    const dir = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'integrations', PACKAGES[harness]);
    if (!existsSync(join(dir, 'package.json'))) {
        throw new Error(`this Co-Review has no ${harness} package (${dir}); update Co-Review`);
    }
    log(`${harness} install ${dir}`);
    const r = spawnSync(harness, ['install', dir], { stdio: 'inherit' });
    if (r.error) {
        throw new Error(r.error.code === 'ENOENT' ? `${harness} is not installed or not on PATH` : r.error.message);
    }
    process.exitCode = r.status ?? 1;
}

(command === 'mcp' ? mcp() : command === 'status' ? status() : command === 'setup' ? setup() : start()).catch(e => {
    log(e.message);
    process.exit(1);
});
