#!/usr/bin/env node
// Co-Review command line.
//
//   co-review [dir]        start Co-Review (or reuse a running one) and open dir in the browser
//   co-review mcp [dir]    MCP server on stdio for agent harnesses; bridges to Co-Review's /mcp endpoint
//                          with dir (default: cwd) as repository, starting Co-Review on the first tool call
//   co-review status [dir] open reviews of dir (default: cwd) in a running Co-Review; never starts it.
//                          --claude-hook prints them as Claude Code SessionStart context (or nothing)
//   co-review setup <pi|omp>
//                          install the Pi or Oh My Pi package that ships with this Co-Review
//                          (runs `pi install` / `omp install` with its path; no clone needed)
//
// Options: --port <n> (default: reuse a running instance, else 3000; or $CO_REVIEW_PORT), --no-open
import { dirname, join, resolve } from 'node:path';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { ensureServer, findServer, home, log, openUrl } from './server.mjs';

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
    const stdio = new StdioServerTransport();
    // The handshake (initialize result, tool list) of the last session, so a new agent session can be
    // answered without Co-Review running: harnesses start this server with every session, and
    // starting the desktop app then would open a window each time. Co-Review starts on the first tool call.
    const cacheFile = join(home, 'mcp-handshake.json');
    let cached;
    try {
        cached = JSON.parse(readFileSync(cacheFile, 'utf8'));
    } catch {
        /* first run */
    }
    const remember = (key, value) => {
        cached = { ...cached, [key]: value };
        try {
            mkdirSync(home, { recursive: true });
            writeFileSync(cacheFile, JSON.stringify(cached));
        } catch {
            /* best effort */
        }
    };

    let http;
    let init;
    const watched = new Map();
    const internal = new Map();
    let nextId = 0;
    const request = (method, params) => new Promise((ok, fail) => {
        const id = `co-review:${nextId++}`;
        internal.set(id, reply => (reply.error ? fail(new Error(reply.error.message)) : ok(reply.result)));
        http.send({ jsonrpc: '2.0', id, method, params }).catch(fail);
    });
    const connect = async () => {
        const base = await ensureServer({ port, root });
        http = new StreamableHTTPClientTransport(new URL(`${base}/mcp?root=${encodeURIComponent(root)}`));
        http.onmessage = message => {
            const own = internal.get(message.id);
            if (own) {
                internal.delete(message.id);
                return own(message);
            }
            // Keep the handshake fresh for the next session.
            const key = watched.get(message.id);
            if (key && message.result) {
                watched.delete(message.id);
                remember(key, message.result);
            }
            stdio.send(message);
        };
        http.onerror = e => log('http:', e.message);
        http.onclose = () => process.exit(0);
        await http.start();
    };
    // Forward JSON-RPC messages both ways; the HTTP transport handles the MCP session id.
    const forward = message => {
        if (message.method === 'initialize' || message.method === 'tools/list') {
            watched.set(message.id, message.method === 'initialize' ? 'initialize' : 'tools');
        }
        http.send(message).catch(e => log('forward failed:', e.message));
    };

    const running = await findServer({ port });
    if (running || !cached?.initialize || !cached?.tools) {
        await connect();
        stdio.onmessage = forward;
    } else {
        // Answer the handshake here; connect (starting Co-Review) when the agent first calls a tool.
        let connecting;
        const answer = (message, result) => stdio.send({ jsonrpc: '2.0', id: message.id, result });
        stdio.onmessage = message => {
            if (!connecting) {
                if (message.method === 'initialize') {
                    init = message;
                    return answer(message, { ...cached.initialize, protocolVersion: message.params.protocolVersion });
                }
                if (message.method === 'tools/list') {
                    return answer(message, cached.tools);
                }
                if (message.method === 'ping') {
                    return answer(message, {});
                }
                if (!('id' in message)) {
                    return;
                }
                connecting = (async () => {
                    await connect();
                    await request('initialize', init.params);
                    await http.send({ jsonrpc: '2.0', method: 'notifications/initialized' });
                    const tools = await request('tools/list', {});
                    if (JSON.stringify(tools) !== JSON.stringify(cached.tools)) {
                        remember('tools', tools);
                        stdio.send({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' });
                    }
                })();
                // Try again on the next call if Co-Review did not start.
                connecting.catch(() => (connecting = undefined));
            }
            connecting.then(() => forward(message), e => {
                log(e.message);
                if ('id' in message) {
                    stdio.send({ jsonrpc: '2.0', id: message.id, error: { code: -32603, message: `Co-Review did not start: ${e.message}` } });
                }
            });
        };
    }
    stdio.onclose = () => (http ? http.close() : process.exit(0));
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
