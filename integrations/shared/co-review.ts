/**
 * Co-Review for Pi and Oh My Pi (omp): the extension both packages load. It starts a Co-Review review
 * for the current repository and stays in it as co-reviewer.
 *
 * - Interactive main session (`/co-review`, `--co-review`, or the `co_review_start` tool): a background
 *   listener waits for the reviewer's questions and hands each one to the session as a message; the
 *   agent answers with `co_review_reply`. Waiting costs no tokens.
 * - Subagents and headless runs: `co_review_start`, then loop `co_review_wait` → investigate →
 *   `co_review_reply`. The blocking wait keeps the subagent resident as co-reviewer.
 *
 * Talks to the running Co-Review app over its local MCP endpoint (started on demand through the
 * `co-review` CLI), so neither harness needs MCP support. What differs per harness (schema builder,
 * how a question is delivered, which session is the main one) comes in as a `Harness`.
 */
import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
// The installed `co-review` command (desktop app or checkout), else this checkout's CLI on the harness's runtime.
const INSTALLED = [join("/usr/local/bin", "co-review"), join(homedir(), ".local", "bin", "co-review")].find(p => existsSync(p));
const CLI = INSTALLED ? [INSTALLED] : [process.execPath, resolve(here, "..", "..", "bin", "co-review.mjs")];
const HOME = process.env.CO_REVIEW_HOME || join(homedir(), ".co-review");

/** The parts of the harness's extension context this extension uses (Pi and omp share them). */
export interface Context {
	cwd: string;
	hasUI: boolean;
	isIdle(): boolean;
	ui: { setStatus(key: string, text: string | undefined): void; notify(message: string, level: "info" | "warning" | "error"): void };
}

/** What differs between Pi and omp. */
export interface Harness<C extends Context = Context> {
	/** How the harness shows up in Co-Review. */
	name: string;
	/** A TypeBox-compatible `Type` builder for tool parameters. */
	Type: any;
	/** Hands a reviewer question to the session. */
	deliver(ctx: C, prompt: string): void;
	/** Whether this is the session the user talks to (it listens and honors `--co-review`), not a subagent. */
	isMain(ctx: C): boolean;
	/** How to keep co-reviewing from a subagent in this harness, for the model's tool guidelines. */
	subagentGuideline: string;
}

interface Thread {
	threadId: string;
	number: number;
	location: { kind: string; path?: string; startLine?: number; endLine?: number; symbol?: string };
	code?: string;
	messages: { author: string; role: "human" | "agent"; body: string }[];
}

interface Opened {
	reviewId: string;
	url?: string;
	note?: string;
	openThreads: number;
	format?: { warnings: string[] };
}

interface Verdict {
	status: string;
	decision?: string;
	summary?: string;
	round?: number;
	comments?: { id: string; where: string; line?: number; body: string }[];
	acceptedSuggestions?: { path?: string; startLine?: number; before: string; after: string }[];
}

type McpClient = import("@modelcontextprotocol/sdk/client/index.js").Client;

/** One connection to Co-Review (an MCP session) for one agent session. */
class CoReviewConnection {
	client: McpClient | undefined;
	listening = false;
	private stopped = false;

	constructor(private readonly clientName: string) {}

	async connect(root: string): Promise<void> {
		if (this.client) {
			return;
		}
		// Start Co-Review (or reuse a running desktop/browser instance) without opening a browser.
		await new Promise<void>((ok, fail) => {
			const child = spawn(CLI[0], [...CLI.slice(1), "--no-open", root], { stdio: ["ignore", "ignore", "pipe"] });
			let err = "";
			child.stderr.on("data", d => (err += d));
			child.on("error", fail);
			child.on("exit", code => (code === 0 ? ok() : fail(new Error(`co-review failed to start: ${err.trim()}`))));
		});
		const { url } = JSON.parse(readFileSync(join(HOME, "server.json"), "utf8")) as { url: string };
		const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
		const { StreamableHTTPClientTransport } = await import("@modelcontextprotocol/sdk/client/streamableHttp.js");
		const client = new Client({ name: this.clientName, version: "1.0.0" });
		await client.connect(new StreamableHTTPClientTransport(new URL(`${url}/mcp?root=${encodeURIComponent(root)}`)));
		this.client = client;
	}

	async call<T = any>(name: string, args: Record<string, unknown>, timeoutMs = 120_000, signal?: AbortSignal): Promise<T> {
		if (!this.client) {
			throw new Error("Not connected to Co-Review. Call co_review_start first.");
		}
		const result = await this.client.callTool({ name, arguments: args }, undefined, { timeout: timeoutMs, signal });
		const text = (result.content as { type: string; text?: string }[]).map(c => c.text ?? "").join("");
		if (result.isError) {
			throw new Error(text);
		}
		// Most tools answer JSON; repo_map answers plain text.
		try {
			return JSON.parse(text) as T;
		} catch {
			return text as T;
		}
	}

	/** Opens (or joins) the review of `root`, or of the review directory `dir`. */
	async open(root: string, opts: { dir?: string; title?: string }): Promise<Opened> {
		await this.connect(root);
		return this.call<Opened>("open_review", {
			root, ...(opts.dir ? { dir: resolve(root, opts.dir) } : {}), ...(opts.title ? { title: opts.title } : {})
		});
	}

	/** Waits for reviewer questions; `undefined` when the wait timed out. */
	async next(timeoutSec: number, signal?: AbortSignal): Promise<Thread[] | undefined> {
		const r = await this.call<{ status: string; threads?: Thread[] }>("await_comment", { timeoutSec }, (timeoutSec + 60) * 1000, signal);
		return r.status === "comment" ? r.threads : undefined;
	}

	async listen(onThreads: (threads: Thread[]) => void, onError: (e: Error) => void): Promise<void> {
		if (this.listening) {
			return;
		}
		this.listening = true;
		this.stopped = false;
		while (!this.stopped) {
			try {
				const threads = await this.next(240);
				if (threads?.length && !this.stopped) {
					onThreads(threads);
				}
			} catch (e) {
				if (this.stopped) {
					break;
				}
				onError(e as Error);
				await new Promise(r => setTimeout(r, 5000));
			}
		}
		this.listening = false;
	}

	async close(): Promise<void> {
		this.stopped = true;
		const client = this.client;
		this.client = undefined;
		await client?.close().catch(() => undefined);
	}
}

/** A reviewer question as a prompt for the agent. */
function formatThread(t: Thread): string {
	const l = t.location;
	const where = l.path ? `${l.path}${l.startLine ? `:${l.startLine}${l.endLine && l.endLine !== l.startLine ? `-${l.endLine}` : ""}` : ""}` : "the repository";
	const lines = [
		`[Co-Review] The reviewer asks in thread #${t.number} (threadId: ${t.threadId}) about ${where}${l.symbol ? ` (${l.symbol})` : ""}.`
	];
	if (t.code) {
		lines.push("", "Code:", "```", t.code, "```");
	}
	lines.push("", "Conversation:");
	for (const m of t.messages) {
		lines.push(`- ${m.role === "agent" ? "You" : `Reviewer (${m.author})`}: ${m.body}`);
	}
	lines.push("", `Investigate as needed, then answer the latest reviewer message by calling co_review_reply with threadId "${t.threadId}". Be concise; reference code as path:line.`);
	return lines.join("\n");
}

function formatVerdict(r: Verdict): string {
	if (r.status !== "submitted") {
		return "Not submitted yet. Keep answering with co_review_wait, or call co_review_verdict again.";
	}
	const comments = (r.comments ?? []).map(c => `- ${c.id} ${c.where}${c.line ? `:${c.line}` : ""}: ${c.body}`).join("\n");
	const suggestions = (r.acceptedSuggestions ?? []).map(s => `- ${s.path}:${s.startLine}\n  - ${s.before}\n  + ${s.after}`).join("\n");
	return [
		`Round ${r.round}: ${r.decision}${r.summary ? ` — "${r.summary}"` : ""}`,
		comments && `Open comments:\n${comments}`,
		suggestions && `Accepted suggestions (apply each as a commit):\n${suggestions}`
	].filter(Boolean).join("\n\n");
}

function text(value: string) {
	return { content: [{ type: "text" as const, text: value }], details: {} };
}

/** Registers the Co-Review flag, command and tools with a Pi or omp extension API. */
export function registerCoReview<C extends Context>(pi: any, harness: Harness<C>): void {
	const { Type } = harness;
	const connection = new CoReviewConnection(harness.name);

	const startListening = (ctx: C) => {
		ctx.ui.setStatus("co-review", "Co-Review: listening");
		void connection.listen(
			threads => threads.forEach(thread => harness.deliver(ctx, formatThread(thread))),
			error => ctx.ui.setStatus("co-review", `Co-Review: reconnecting (${error.message.slice(0, 60)})`)
		);
	};

	const start = async (ctx: C, opts: { root?: string; dir?: string; title?: string; listen: boolean }) => {
		const opened = await connection.open(resolve(opts.root ?? ctx.cwd), opts);
		if (opts.listen) {
			startListening(ctx);
		}
		return opened;
	};

	pi.registerFlag("co-review", {
		description: "Start a Co-Review review for the working directory and stay as co-reviewer",
		type: "boolean",
		default: false
	});

	pi.on("session_start", async (_event: unknown, ctx: C) => {
		if (pi.getFlag("co-review") && harness.isMain(ctx)) {
			const opened = await start(ctx, { listen: ctx.hasUI });
			ctx.ui.notify(`Co-Review: co-reviewing ${opened.url ?? "(desktop app)"}`, "info");
		}
	});

	pi.on("session_shutdown", async () => {
		await connection.close();
	});

	pi.registerCommand("co-review", {
		description: "Open a Co-Review review of this repository and stay as co-reviewer (/co-review stop to leave)",
		handler: async (args: string, ctx: C) => {
			if (args.trim() === "stop") {
				await connection.close();
				ctx.ui.setStatus("co-review", undefined);
				ctx.ui.notify("Co-Review: left the review", "info");
				return;
			}
			const opened = await start(ctx, { listen: true, title: args.trim() || undefined });
			ctx.ui.notify(`Co-Review: co-reviewing ${opened.url ?? "(shown in the desktop app)"} — ${opened.openThreads} open threads`, "info");
		}
	});

	pi.registerTool({
		name: "co_review_start",
		label: "Co-Review: start",
		description: "Open a Co-Review review of a repository (default: the working directory), or of a review directory (a design document and/or patches, `dir`), and join it as co-reviewer.",
		promptSnippet: "Open a Co-Review code review with the user and act as co-reviewer",
		promptGuidelines: [
			"Use co_review_start when the user asks to review code together in Co-Review; afterwards answer reviewer questions with co_review_reply.",
			`In the main interactive session, reviewer questions arrive as [Co-Review] messages after co_review_start. ${harness.subagentGuideline}`
		],
		parameters: Type.Object({
			root: Type.Optional(Type.String({ description: "Absolute repository path; defaults to the working directory" })),
			dir: Type.Optional(Type.String({ description: "Review directory with a design document (index.markdown) and/or *.patch files, relative to root or absolute" })),
			title: Type.Optional(Type.String({ description: "Title for a new review; omit to join the latest one" }))
		}),
		async execute(_id: string, params: { root?: string; dir?: string; title?: string }, _signal: AbortSignal, _onUpdate: unknown, ctx: C) {
			const listen = ctx.hasUI && harness.isMain(ctx);
			const opened = await start(ctx, { ...params, listen });
			const warnings = opened.format?.warnings?.length ? `\nFix the design document, then call co_review_start again: ${opened.format.warnings.join(" ")}` : "";
			const where = (opened.url ? `Reviewer URL: ${opened.url}` : opened.note ?? "") + warnings;
			return text(listen
				? `Joined review ${opened.reviewId} (${opened.openThreads} open threads). ${where}\nReviewer questions will arrive as [Co-Review] messages; answer each with co_review_reply.`
				: `Joined review ${opened.reviewId} (${opened.openThreads} open threads). ${where}\nNow loop: co_review_wait → investigate → co_review_reply, until the reviewer says the review is done.`);
		}
	});

	pi.registerTool({
		name: "co_review_wait",
		label: "Co-Review: wait",
		description: "Block until the reviewer asks a question or replies in a thread you are part of. Returns the questions to answer.",
		parameters: Type.Object({
			timeoutSec: Type.Optional(Type.Number({ description: "Seconds to wait (default 600)" }))
		}),
		async execute(_id: string, params: { timeoutSec?: number }, signal: AbortSignal) {
			const threads = await connection.next(params.timeoutSec ?? 600, signal);
			return text(threads?.length
				? threads.map(formatThread).join("\n\n---\n\n")
				: "No questions yet. Call co_review_wait again to keep co-reviewing.");
		}
	});

	pi.registerTool({
		name: "co_review_reply",
		label: "Co-Review: reply",
		description: "Answer the reviewer in a Co-Review thread (markdown). Answer first, in plain language, in a few sentences; no line-number walkthroughs (at most one or two path:line links at the end).",
		parameters: Type.Object({
			threadId: Type.String(),
			body: Type.String({ description: "Markdown answer" }),
			resolve: Type.Optional(Type.Boolean({ description: "Also resolve the thread" }))
		}),
		async execute(_id: string, params: Record<string, unknown>) {
			await connection.call("reply", params);
			return text("Replied in the review.");
		}
	});

	pi.registerTool({
		name: "co_review_map",
		label: "Co-Review: repository map",
		description: "Outline of the repository: every source file with the classes, functions and methods it defines. Use it to find where something lives before reading or searching files.",
		parameters: Type.Object({
			path: Type.Optional(Type.String({ description: "Only files under this repository-relative folder (or this file)" })),
			query: Type.Optional(Type.String({ description: "Only files whose path or symbol names contain this" }))
		}),
		async execute(_id: string, params: Record<string, unknown>) {
			return text(await connection.call<string>("repo_map", params));
		}
	});

	pi.registerTool({
		name: "co_review_add_findings",
		label: "Co-Review: add findings",
		description: "Add review findings as threads on code (lines are 1-based; without a line, on the file or folder). labels group them in the panel (e.g. the area); status \"proposed\" lets the reviewer accept or dismiss each.",
		parameters: Type.Object({
			findings: Type.Array(Type.Object({
				path: Type.String({ description: "File or folder path relative to the repository (\".\" for the repository)" }),
				line: Type.Optional(Type.Number()),
				endLine: Type.Optional(Type.Number()),
				body: Type.String({ description: "Markdown" }),
				severity: Type.Optional(Type.Union([Type.Literal("low"), Type.Literal("medium"), Type.Literal("high")])),
				labels: Type.Optional(Type.Array(Type.String(), { description: "The first label is the area the panel groups by" })),
				status: Type.Optional(Type.Union([Type.Literal("open"), Type.Literal("proposed")]))
			}))
		}),
		async execute(_id: string, params: Record<string, unknown>) {
			const r = await connection.call<{ created: number; ids: string[] }>("add_findings", params);
			return text(`Added ${r.created} finding(s): ${r.ids.join(", ")}`);
		}
	});

	pi.registerTool({
		name: "co_review_verdict",
		label: "Co-Review: verdict",
		description: "Wait for the reviewer to submit the review (Approve / Request changes / Comment). Returns the decision, their message, the open comments and accepted suggestions to apply.",
		parameters: Type.Object({
			timeoutSec: Type.Optional(Type.Number({ description: "Seconds to wait (default 300)" }))
		}),
		async execute(_id: string, params: { timeoutSec?: number }, signal: AbortSignal) {
			const timeoutSec = params.timeoutSec ?? 300;
			return text(formatVerdict(await connection.call<Verdict>("await_review", { timeoutSec }, (timeoutSec + 30) * 1000, signal)));
		}
	});

	pi.registerTool({
		name: "co_review_ask",
		label: "Co-Review: ask reviewer",
		description: "Ask the reviewer to choose between options (shown as buttons in the review). Blocks until answered.",
		parameters: Type.Object({
			question: Type.String(),
			options: Type.Array(Type.String(), { minItems: 1, maxItems: 6 }),
			threadId: Type.Optional(Type.String())
		}),
		async execute(_id: string, params: Record<string, unknown>, signal: AbortSignal) {
			const r = await connection.call<{ status: string; choice?: string }>("ask_reviewer", params, 960_000, signal);
			return text(r.status === "answered" ? `The reviewer chose: ${r.choice}` : "The reviewer has not answered yet.");
		}
	});
}
