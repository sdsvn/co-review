#!/usr/bin/env node
// Review server mode: opens a review directory (or a document / patch) in Co-Review and keeps a
// JSON state file of the review, for scripts and workflows that drive a review from outside:
//
//   co-review-server [-dir <dir> | <dir>] [-md <file>] [-patch <file>] [-addr host:port] [-store <review.json>] [-openspec <dir>]
//   co-review-server mcp         same as `co-review mcp`
//
// This process serves a small HTTP API on -addr (`/` redirects to Co-Review, `/api/context`, thread
// replies, findings), and Co-Review keeps -store up to date; `.review.count` increments on each Submit.
import { readFileSync } from 'node:fs';
import http from 'node:http';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { ensureServer, log } from './server.mjs';

const here = dirname(fileURLToPath(import.meta.url));

if (process.argv[2] === 'mcp') {
    spawn(process.execPath, [join(here, 'co-review.mjs'), 'mcp', ...process.argv.slice(3)], { stdio: 'inherit' })
        .on('exit', code => process.exit(code ?? 0));
} else {
    main().catch(e => {
        log(e.message);
        process.exit(1);
    });
}

function flags(argv) {
    const f = { dir: '', md: '', patch: '', store: '', addr: ':8080', author: 'you', openspec: '', positional: '' };
    for (let i = 0; i < argv.length; i++) {
        const arg = argv[i];
        if (!arg.startsWith('-')) {
            f.positional = arg;
            continue;
        }
        let key = arg.replace(/^--?/, '');
        let value;
        if (key.includes('=')) {
            [key, value] = [key.slice(0, key.indexOf('=')), key.slice(key.indexOf('=') + 1)];
        } else if (key !== 'dev') {
            value = argv[++i];
        }
        if (key === 'd') {
            key = 'dir';
        }
        if (key in f) {
            f[key] = value ?? '';
        }
    }
    if (!f.dir && f.positional) {
        f.dir = f.positional;
    }
    return f;
}

async function main() {
    const f = flags(process.argv.slice(2));
    const dir = f.dir ? resolve(f.dir) : undefined;
    const root = dir ?? dirname(resolve(f.md || f.patch || '.'));
    const base = await ensureServer({ root });

    const { Client } = await import('@modelcontextprotocol/sdk/client/index.js');
    const { StreamableHTTPClientTransport } = await import('@modelcontextprotocol/sdk/client/streamableHttp.js');
    const client = new Client({ name: 'agent', version: '1.0.0' });
    await client.connect(new StreamableHTTPClientTransport(new URL(`${base}/mcp?root=${encodeURIComponent(root)}`)));
    const call = async (name, args, timeout = 120000) => {
        const r = await client.callTool({ name, arguments: args }, undefined, { timeout });
        const text = r.content.map(c => c.text ?? '').join('');
        if (r.isError) {
            throw new Error(text);
        }
        return JSON.parse(text);
    };

    const opened = await call('open_review', {
        ...(dir ? { dir } : {}),
        ...(f.md ? { markdown: readFileSync(resolve(f.md), 'utf8'), title: basename(f.md) } : {}),
        ...(f.patch ? { patch: readFileSync(resolve(f.patch), 'utf8'), patchName: basename(f.patch).replace(/\.(patch|diff)$/, '') } : {}),
        ...(f.store ? { storePath: resolve(f.store) } : {}),
        ...(f.openspec ? { openspec: resolve(f.openspec) } : {}),
        open: false
    });
    const reviewUrl = opened.url ?? base;
    const reviewDir = opened.dir ?? dir;
    const store = f.store ? resolve(f.store) : join(reviewDir, 'review.json');
    const state = () => {
        try {
            return JSON.parse(readFileSync(store, 'utf8'));
        } catch {
            return { doc: {}, threads: [], review: { count: 0 } };
        }
    };

    const body = req => new Promise((ok, fail) => {
        let data = '';
        req.on('data', c => (data += c));
        req.on('end', () => {
            try {
                ok(data ? JSON.parse(data) : {});
            } catch (e) {
                fail(e);
            }
        });
    });
    const send = (res, status, value) => {
        res.writeHead(status, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify(value));
    };

    const server = http.createServer(async (req, res) => {
        const url = new URL(req.url ?? '/', 'http://localhost');
        const p = url.pathname;
        try {
            if (p === '/api/context' && req.method === 'GET') {
                const s = state();
                return send(res, 200, { doc: s.doc, patches: [], hasDoc: !!s.doc?.markdown, author: f.author, threads: s.threads, review: s.review, openspec: null, url: reviewUrl });
            }
            const reply = p.match(/^\/api\/threads\/([^/]+)\/messages$/);
            if (reply && req.method === 'POST') {
                const b = await body(req);
                return send(res, 200, await call('reply', { threadId: decodeURIComponent(reply[1]), body: String(b.body ?? '') }));
            }
            if (p === '/api/findings' && req.method === 'POST') {
                const b = await body(req);
                return send(res, 200, await call('add_findings', { findings: Array.isArray(b) ? b : [] }));
            }
            if (p.startsWith('/api/')) {
                return send(res, 501, { error: `${p} is handled in Co-Review (${reviewUrl})` });
            }
            // The review UI is Co-Review.
            res.writeHead(302, { Location: reviewUrl });
            res.end();
        } catch (e) {
            send(res, 400, { error: e.message });
        }
    });

    const [host, portText] = f.addr.includes(':') ? [f.addr.slice(0, f.addr.lastIndexOf(':')), f.addr.slice(f.addr.lastIndexOf(':') + 1)] : ['', f.addr];
    await new Promise((ok, fail) => {
        server.once('error', fail);
        server.listen(Number(portText) || 0, host || '127.0.0.1', ok);
    });
    const address = server.address();
    const files = opened.patches ?? 0;
    // Startup line with the URL; callers wait for it.
    console.log(`review server on http://${address.address}:${address.port} — doc: ${state().doc?.slug || '-'}, patches: ${files}, store: ${store} — Co-Review: ${reviewUrl}`);

    const shutdown = async () => {
        server.close();
        await client.close().catch(() => undefined);
        process.exit(0);
    };
    process.on('SIGTERM', shutdown);
    process.on('SIGINT', shutdown);
}
