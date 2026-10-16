// The `co-review mcp` bridge (bin/co-review.mjs): an agent's calls survive Co-Review forgetting the session.
// Run: `node --test extensions/review/test` (make test). Needs the repository's node_modules (the MCP SDK, express).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { randomUUID } from 'node:crypto';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { z } from 'zod';

const require = createRequire(import.meta.url);
const express = require('express');
const { McpServer } = require('@modelcontextprotocol/sdk/server/mcp.js');
const { StreamableHTTPServerTransport } = require('@modelcontextprotocol/sdk/server/streamableHttp.js');
const { isInitializeRequest } = require('@modelcontextprotocol/sdk/types.js');
const bridge = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'bin', 'co-review.mjs');
/** The bridge's version (a checkout's: its app's package.json), which its handshake cache is stamped with. */
const version = JSON.parse(readFileSync(resolve(dirname(bridge), '..', 'applications', 'electron', 'package.json'), 'utf8')).version;

/** A Co-Review-like MCP server with one tool, that can be told to forget every session. */
function fakeCoReview() {
    const sessions = new Map();
    const stats = { initializes: 0, calls: 0 };
    const app = express();
    app.all('/mcp', express.json(), async (req, res) => {
        const id = req.headers['mcp-session-id'];
        let transport = id ? sessions.get(id) : undefined;
        if (!transport) {
            if (req.method !== 'POST' || !isInitializeRequest(req.body)) {
                return res.status(id ? 404 : 400).json({ jsonrpc: '2.0', error: { code: -32000, message: 'No valid MCP session' }, id: null });
            }
            stats.initializes++;
            transport = new StreamableHTTPServerTransport({ sessionIdGenerator: () => randomUUID(), onsessioninitialized: sid => sessions.set(sid, transport) });
            const server = new McpServer({ name: 'fake', version: '0' });
            server.registerTool('echo', { inputSchema: { text: z.string() } }, async ({ text }) => {
                stats.calls++;
                return { content: [{ type: 'text', text: `echo:${text}` }] };
            });
            await server.connect(transport);
        }
        await transport.handleRequest(req, res, req.body);
    });
    return new Promise(ok => {
        const listener = app.listen(0, '127.0.0.1', () => ok({ port: listener.address().port, stats, forget: () => sessions.clear(), close: () => listener.close() }));
    });
}

/** The bridge as an agent's harness sees it: JSON-RPC lines over stdio. */
function agent(port, home) {
    const child = spawn(process.execPath, [bridge, 'mcp', '--port', String(port), process.cwd()], { env: { ...process.env, CO_REVIEW_HOME: home }, stdio: ['pipe', 'pipe', 'pipe'] });
    const waiting = new Map();
    let buffer = '';
    child.stdout.on('data', chunk => {
        buffer += chunk;
        let at;
        while ((at = buffer.indexOf('\n')) >= 0) {
            const line = buffer.slice(0, at).trim();
            buffer = buffer.slice(at + 1);
            if (!line) {
                continue;
            }
            const message = JSON.parse(line);
            if ('id' in message) {
                waiting.get(message.id)?.(message);
                waiting.delete(message.id);
            }
        }
    });
    let n = 0;
    const send = (method, params) => new Promise((ok, fail) => {
        const id = ++n;
        const timer = setTimeout(() => fail(new Error(`${method} not answered in 10 s`)), 10_000);
        waiting.set(id, message => {
            clearTimeout(timer);
            ok(message);
        });
        child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    });
    return {
        send,
        notify: (method, params) => child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`),
        call: async text => {
            const reply = await send('tools/call', { name: 'echo', arguments: { text } });
            return reply.error ? `error: ${reply.error.message}` : reply.result.content[0].text;
        },
        end: () => child.kill('SIGKILL')
    };
}

test('a call after Co-Review forgot the session is answered on a new session, once, without the agent noticing', async t => {
    const home = mkdtempSync(join(tmpdir(), 'co-review-bridge-'));
    const server = await fakeCoReview();
    const a = agent(server.port, home);
    t.after(() => {
        a.end();
        server.close();
        rmSync(home, { recursive: true, force: true });
    });
    await a.send('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'test', version: '0' } });
    a.notify('notifications/initialized');
    assert.equal(await a.call('one'), 'echo:one');
    assert.deepEqual(server.stats, { initializes: 1, calls: 1 });

    server.forget();
    assert.equal(await a.call('two'), 'echo:two');
    assert.equal(await a.call('three'), 'echo:three');
    assert.deepEqual(server.stats, { initializes: 2, calls: 3 }, 'one new session, every call ran once');

    // Two calls in flight when the session goes: both come back, on one new session.
    server.forget();
    assert.deepEqual(await Promise.all([a.call('four'), a.call('five')]), ['echo:four', 'echo:five']);
    assert.deepEqual(server.stats, { initializes: 3, calls: 5 });
});

test('a probe before initialize (Claude Code sends server/discover) is not found, and does not cost the session', async t => {
    const home = mkdtempSync(join(tmpdir(), 'co-review-bridge-'));
    const server = await fakeCoReview();
    const a = agent(server.port, home);
    t.after(() => {
        a.end();
        server.close();
        rmSync(home, { recursive: true, force: true });
    });
    assert.equal((await a.send('server/discover', {})).error?.code, -32601);
    await a.send('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '0' } });
    a.notify('notifications/initialized');
    assert.equal(await a.call('one'), 'echo:one');
    assert.deepEqual(server.stats, { initializes: 1, calls: 1 });
});

test('a probe before initialize does not start Co-Review; the handshake comes from the cache', async t => {
    const home = mkdtempSync(join(tmpdir(), 'co-review-bridge-'));
    writeFileSync(join(home, 'mcp-handshake.json'), JSON.stringify({
        initialize: { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'co-review', version: '0.1.0' } },
        tools: { tools: [] },
        version
    }));
    // Nothing listens on this port: the bridge would start Co-Review there.
    const server = await fakeCoReview();
    server.close();
    const a = agent(server.port, home);
    t.after(() => {
        a.end();
        rmSync(home, { recursive: true, force: true });
    });
    assert.equal((await a.send('server/discover', {})).error?.code, -32601);
    const init = await a.send('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '0' } });
    assert.equal(init.result?.serverInfo?.name, 'co-review');
    a.notify('notifications/initialized');
    assert.deepEqual((await a.send('tools/list', {})).result, { tools: [] });
    const log = join(home, 'logs', 'cli.log');
    assert.doesNotMatch(existsSync(log) ? readFileSync(log, 'utf8') : '', /starting Co-Review|lost Co-Review/);
});

test('after an update the handshake of the old version is not used: the tools come from Co-Review, and are cached', async t => {
    const home = mkdtempSync(join(tmpdir(), 'co-review-bridge-'));
    writeFileSync(join(home, 'mcp-handshake.json'), JSON.stringify({
        initialize: { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'co-review', version: '0.1.0' } },
        tools: { tools: [] },
        version: '0.0.1'
    }));
    const server = await fakeCoReview();
    const a = agent(server.port, home);
    t.after(() => {
        a.end();
        server.close();
        rmSync(home, { recursive: true, force: true });
    });
    const init = await a.send('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'test', version: '0' } });
    assert.equal(init.result?.serverInfo?.name, 'fake');
    a.notify('notifications/initialized');
    assert.deepEqual((await a.send('tools/list', {})).result.tools.map(tool => tool.name), ['echo']);
    const cached = JSON.parse(readFileSync(join(home, 'mcp-handshake.json'), 'utf8'));
    assert.equal(cached.version, version);
    assert.deepEqual(cached.tools.tools.map(tool => tool.name), ['echo']);
});
