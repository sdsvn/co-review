// The `co-review mcp` bridge (bin/co-review.mjs): an agent's calls survive Co-Review forgetting the session.
// Run: `node --test extensions/review/test` (make test). Needs the repository's node_modules (the MCP SDK, express).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
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
