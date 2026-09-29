#!/usr/bin/env node
// Co-Review command line: see USAGE (`co-review help`).
import { dirname, join, resolve } from 'node:path';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { ensureServer, findServer, home, log, openUrl } from './server.mjs';

const USAGE = `Usage: co-review [command] [dir] [options]

Commands:
  (none) [dir]       start Co-Review (or reuse a running one) and open dir (default: the current directory)
  mcp [dir]          MCP server on stdio for agent harnesses; bridges to Co-Review's /mcp endpoint with dir
                     (default: cwd) as repository, starting Co-Review on the first tool call
  status [dir]       open reviews of dir in a running Co-Review; never starts it
                     (--claude-hook: as Claude Code SessionStart context, or nothing)
  setup <pi|omp>     install the Pi or Oh My Pi package that ships with this Co-Review
  version            print the version
  help               print this help

Options:
  --port <n>         port of the Co-Review to use or start (default: a running one, else 3000; or $CO_REVIEW_PORT)
  --no-open          start without opening a browser or window
  -v, --version      print the version
  -h, --help         print this help
`;

/** This Co-Review's version: the desktop app's package.json, or the checkout's app. */
function version() {
    const here = dirname(fileURLToPath(import.meta.url));
    for (const file of [join(here, '..', 'package.json'), join(here, '..', 'applications', 'electron', 'package.json')]) {
        try {
            const { version } = JSON.parse(readFileSync(file, 'utf8'));
            if (version) {
                return version;
            }
        } catch {
            /* next */
        }
    }
    return 'unknown';
}

const args = process.argv.slice(2);
if (['help', '--help', '-h'].includes(args[0]) || args.includes('--help') || args.includes('-h')) {
    process.stdout.write(USAGE);
    process.exit(0);
}
if (['version', '--version', '-v'].includes(args[0])) {
    console.log(`co-review ${version()}`);
    process.exit(0);
}
const take = name => {
    const i = args.indexOf(name);
    return i < 0 ? undefined : args.splice(i, 2)[1];
};
const port = Number(take('--port') ?? process.env.CO_REVIEW_PORT ?? 0) || undefined;
const noOpen = args.includes('--no-open') && !!args.splice(args.indexOf('--no-open'), 1);
const claudeHook = args.includes('--claude-hook') && !!args.splice(args.indexOf('--claude-hook'), 1);
const unknown = args.find(a => a.startsWith('-'));
if (unknown) {
    process.stderr.write(`co-review: unknown option ${unknown}\n\n${USAGE}`);
    process.exit(2);
}
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

    // Co-Review can go away under a session (the reviewer quits the app, or it closes itself when the review is
    // done). The agent's calls then fail with an error instead of waiting forever, and the next one reconnects,
    // starting Co-Review again and replaying the agent's `initialize`.
    let http;
    /** The connection being made or made: every message to Co-Review waits for it. Reset when Co-Review is lost. */
    let connection;
    let init;
    /** Agent requests forwarded to Co-Review and not answered yet. */
    const inflight = new Set();
    const watched = new Map();
    const internal = new Map();
    let nextId = 0;
    const request = (method, params, timeoutMs = 30_000) => new Promise((ok, fail) => {
        const id = `co-review:${nextId++}`;
        const timer = setTimeout(() => internal.get(id)?.({ error: { message: `${method} timed out` } }), timeoutMs);
        internal.set(id, reply => {
            clearTimeout(timer);
            internal.delete(id);
            return reply.error ? fail(new Error(reply.error.message)) : ok(reply.result);
        });
        http.send({ jsonrpc: '2.0', id, method, params }).catch(e => internal.get(id)?.({ error: { message: e.message } }));
    });
    const lost = why => {
        if (!http) {
            return;
        }
        log(`lost Co-Review: ${why}`);
        const dead = http;
        http = undefined;
        connection = undefined;
        dead.close().catch(() => undefined);
        for (const id of inflight) {
            stdio.send({ jsonrpc: '2.0', id, error: { code: -32603, message: `Co-Review closed (${why}). Call the tool again: it reconnects.` } });
        }
        inflight.clear();
        for (const reply of [...internal.values()]) {
            reply({ error: { message: why } });
        }
    };
    // Transport errors are also reported for streams the SDK resumes by itself: check that the session still answers.
    let checking = false;
    const check = async () => {
        if (checking || !http) {
            return;
        }
        checking = true;
        try {
            await request('ping', {}, 10_000);
        } catch (e) {
            lost(e.message);
        } finally {
            checking = false;
        }
    };
    const connect = async replay => {
        const base = await ensureServer({ port, root });
        const transport = new StreamableHTTPClientTransport(new URL(`${base}/mcp?root=${encodeURIComponent(root)}`));
        transport.onmessage = message => {
            const own = internal.get(message.id);
            if (own) {
                return own(message);
            }
            if ('id' in message && !('method' in message)) {
                inflight.delete(message.id);
            }
            // Keep the handshake fresh for the next session.
            const key = watched.get(message.id);
            if (key && message.result) {
                watched.delete(message.id);
                remember(key, message.result);
            }
            stdio.send(message);
        };
        transport.onerror = e => {
            log('http:', e.message);
            if (transport === http) {
                check();
            }
        };
        await transport.start();
        http = transport;
        if (replay) {
            await request('initialize', init.params);
            await http.send({ jsonrpc: '2.0', method: 'notifications/initialized' });
            const tools = await request('tools/list', {});
            if (JSON.stringify(tools) !== JSON.stringify(cached?.tools)) {
                remember('tools', tools);
                stdio.send({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' });
            }
        }
    };
    const answer = (message, result) => stdio.send({ jsonrpc: '2.0', id: message.id, result });

    // With Co-Review running (or nothing cached yet) connect now and forward the handshake; otherwise answer it here.
    if (await findServer({ port }) || !cached?.initialize || !cached?.tools) {
        connection = connect(false);
        connection.catch(() => (connection = undefined));
    }
    stdio.onmessage = message => {
        const handshake = message.method === 'initialize';
        if (handshake) {
            init = message;
        }
        if (!connection && cached?.initialize && cached?.tools) {
            if (handshake) {
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
        }
        if (!connection) {
            connection = connect(!!init && !handshake);
            // Try again on the next call if Co-Review did not start.
            connection.catch(() => (connection = undefined));
        }
        const isRequest = 'id' in message && 'method' in message;
        if (isRequest) {
            inflight.add(message.id);
        }
        if (handshake || message.method === 'tools/list') {
            watched.set(message.id, handshake ? 'initialize' : 'tools');
        }
        connection.then(() => http.send(message)).catch(e => {
            if (http) {
                lost(e.message);
            } else {
                log(e.message);
            }
            if (isRequest && inflight.delete(message.id)) {
                stdio.send({ jsonrpc: '2.0', id: message.id, error: { code: -32603, message: `Co-Review is not reachable: ${e.message}` } });
            }
        });
    };
    // The agent's session ended (the harness closed stdin, which the stdio transport does not report): end ours
    // too, so Co-Review knows this agent is done, and exit instead of lingering on the open HTTP stream.
    const end = async () => {
        if (http) {
            await Promise.race([http.terminateSession().catch(() => undefined), new Promise(r => setTimeout(r, 2000))]);
        }
        process.exit(0);
    };
    stdio.onclose = end;
    process.stdin.on('end', end);
    process.stdout.on('error', end);
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
