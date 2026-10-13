/**
 * The Pi / Oh My Pi extension against a fake `co-review mcp` bridge (JSON-RPC lines on stdio):
 *   node --experimental-transform-types --test integrations/shared/co-review.test.ts
 */
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const scratch = mkdtempSync(join(tmpdir(), "co-review-extension-"));
const received = join(scratch, "received.jsonl");
const bridge = join(scratch, "co-review");
// Answers like Co-Review behind `co-review mcp`; `await_reviewer` with timeoutSec 999 never answers, `repo_map` ends
// the bridge.
writeFileSync(bridge, `#!/usr/bin/env node
const { appendFileSync } = require("node:fs");
const text = value => ({ content: [{ type: "text", text: JSON.stringify(value) }] });
const send = message => process.stdout.write(JSON.stringify(message) + "\\n");
require("node:readline").createInterface({ input: process.stdin }).on("line", line => {
	const message = JSON.parse(line);
	appendFileSync(${JSON.stringify(received)}, JSON.stringify({ argv: process.argv.slice(2), message }) + "\\n");
	const { id, method, params } = message;
	if (method === "initialize") {
		return send({ jsonrpc: "2.0", id, result: { protocolVersion: params.protocolVersion, capabilities: {}, serverInfo: { name: "co-review", version: "0" } } });
	}
	if (method !== "tools/call") {
		return;
	}
	const { name, arguments: args } = params;
	if (name === "repo_map") {
		process.exit(3);
	}
	if (name === "open_review") {
		return send({ jsonrpc: "2.0", id, result: text({ reviewId: "r1", openThreads: 0, note: 'Prepared. Add findings, then call open_review({ reviewId: "r1" }) to show it.' }) });
	}
	if (name === "await_reviewer" && args.timeoutSec !== 999) {
		send({ jsonrpc: "2.0", method: "notifications/claude/channel", params: { content: "not for the extension" } });
		return send({ jsonrpc: "2.0", id, result: text({ status: "comment", threads: [{ threadId: "th-1", number: 1,
			location: { kind: "line", path: "calc.py", startLine: 3 }, messages: [{ author: "Ann", role: "human", body: "Why zero?" }] }] }) });
	}
});
`);
chmodSync(bridge, 0o755);
process.env.CO_REVIEW_CLI = bridge;
const { registerCoReview } = await import("./co-review.ts");

type Tool = { name: string; execute(id: string, params: any, signal?: AbortSignal, onUpdate?: unknown, ctx?: unknown): Promise<any> };

function extension() {
	const tools = new Map<string, Tool>();
	const handlers = new Map<string, (...args: any[]) => unknown>();
	const pi = {
		registerFlag() {},
		registerCommand() {},
		getFlag: () => false,
		on: (event: string, handler: (...args: any[]) => unknown) => handlers.set(event, handler),
		registerTool: (tool: Tool) => tools.set(tool.name, tool)
	};
	registerCoReview(pi, { name: "Test", Type: new Proxy({}, { get: () => () => ({}) }), deliver() {}, isMain: () => true, subagentGuideline: "" });
	const ctx = { cwd: scratch, hasUI: false, isIdle: () => true, ui: { setStatus() {}, notify() {} } };
	const run = async (name: string, params: object, signal?: AbortSignal) =>
		(await tools.get(name)!.execute("call", params, signal, undefined, ctx)).content[0].text as string;
	return { run, shutdown: () => handlers.get("session_shutdown")?.() };
}

const sent = () => readFileSync(received, "utf8").trim().split("\n").map(line => JSON.parse(line));

test("a review is opened through `co-review mcp` and the reviewer's question comes back from co_review_wait", async () => {
	const { run, shutdown } = extension();
	const opened = await run("co_review_start", { open: false });
	assert.match(opened, /^Joined review r1 \(0 open threads\)/);
	assert.match(opened, /call co_review_start\(\{ reviewId: "r1" \}\)/, "Co-Review's tool names are this extension's");
	const asked = await run("co_review_wait", { timeoutSec: 5 });
	assert.match(asked, /thread #1 \(threadId: th-1\) about calc\.py:3/);
	assert.match(asked, /Reviewer \(Ann\): Why zero\?/);
	await shutdown();
	const first = sent()[0];
	assert.deepEqual(first.argv, ["mcp", scratch]);
	assert.equal(first.message.method, "initialize");
});

test("an aborted wait is cancelled at Co-Review, and a bridge that ends fails the call", async () => {
	const { run, shutdown } = extension();
	await run("co_review_start", { open: false });
	const abort = new AbortController();
	const waiting = run("co_review_wait", { timeoutSec: 999 }, abort.signal);
	setTimeout(() => abort.abort(), 100);
	await assert.rejects(waiting, /Cancelled/);
	await new Promise(done => setTimeout(done, 100));
	const cancelled = sent().at(-1).message;
	assert.equal(cancelled.method, "notifications/cancelled");
	await assert.rejects(run("co_review_map", {}), /co-review mcp ended \(exit 3\)/);
	// The next call starts a new bridge.
	assert.match(await run("co_review_start", { open: false }), /^Joined review r1/);
	await shutdown();
});
