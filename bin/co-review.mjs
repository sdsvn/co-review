#!/usr/bin/env node
// Co-Review command line.
//
//   co-review [dir]        start Co-Review (or reuse a running one) and open dir in the browser
//   co-review mcp [dir]    MCP server on stdio for agent harnesses; starts Co-Review on demand
//                          and bridges to its /mcp endpoint with dir (default: cwd) as repository
//
// Options: --port <n> (default: reuse a running instance, else 3000; or $CO_REVIEW_PORT), --no-open
import { resolve } from 'node:path';
import { ensureServer, log, openUrl } from './server.mjs';

const args = process.argv.slice(2);
const take = name => {
    const i = args.indexOf(name);
    return i < 0 ? undefined : args.splice(i, 2)[1];
};
const port = Number(take('--port') ?? process.env.CO_REVIEW_PORT ?? 0) || undefined;
const noOpen = args.includes('--no-open') && !!args.splice(args.indexOf('--no-open'), 1);
const command = args[0] === 'mcp' ? args.shift() : 'start';
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

(command === 'mcp' ? mcp() : start()).catch(e => {
    log(e.message);
    process.exit(1);
});
