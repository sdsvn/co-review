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
 * Talks to Co-Review through `co-review mcp` (the stdio bridge every MCP harness uses: it starts Co-Review on the
 * first tool call and reconnects when it restarts), in plain JSON-RPC lines, so neither harness needs MCP support and
 * the package needs no dependencies. What differs per harness (schema builder, how a question is delivered, which
 * session is the main one) comes in as a `Harness`.
 */
import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
// The `co-review` command: `$CO_REVIEW_CLI`, else the one that came with this package — bundled with the app it came
// from (<resources>/app/integrations/shared → <resources>/app/bin) or built in this checkout — so the package always
// talks to its own Co-Review, never to an older one still linked in ~/.local/bin; else the one installed (install.sh
// links it), else `co-review` on PATH (the Windows installer puts its folder there).
const EXE = process.platform === "win32" ? "co-review.exe" : "co-review";
const CLI =
	[
		process.env.CO_REVIEW_CLI ?? "",
		resolve(here, "..", "..", "bin", EXE),
		resolve(here, "..", "..", "target", "debug", EXE),
		resolve(here, "..", "..", "target", "release", EXE),
		join("/usr/local/bin", EXE),
		join(homedir(), ".local", "bin", EXE),
	].find(p => p && existsSync(p)) ?? "co-review";

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
	/** With status "comment": reviewer comments that came before the Submit. */
	threads?: Thread[];
	/** With status "closed": what to do now that the reviewer closed the review. */
	hint?: string;
	decision?: string;
	summary?: string;
	round?: number;
	comments?: { id: string; where: string; line?: number; body: string }[];
	acceptedSuggestions?: { path?: string; startLine?: number; before: string; after: string }[];
}

interface ToolResult {
	content?: { type: string; text?: string }[];
	isError?: boolean;
}

interface Pending {
	resolve(result: ToolResult): void;
	reject(error: Error): void;
}

/** One connection to Co-Review (a `co-review mcp` bridge, so an MCP session) for one agent session. */
class CoReviewConnection {
	private bridge: ChildProcessWithoutNullStreams | undefined;
	private ready: Promise<void> | undefined;
	private readonly pending = new Map<number, Pending>();
	private nextId = 0;
	listening = false;
	private stopped = false;

	constructor(private readonly clientName: string) {}

	get connected(): boolean {
		return this.bridge !== undefined;
	}

	connect(root: string): Promise<void> {
		this.ready ??= this.start(root).catch(error => {
			this.drop(error as Error);
			throw error;
		});
		return this.ready;
	}

	private async start(root: string): Promise<void> {
		const bridge = spawn(CLI, ["mcp", root], { stdio: ["pipe", "pipe", "pipe"] });
		this.bridge = bridge;
		let stderr = "";
		bridge.stderr.on("data", d => (stderr = (stderr + d).slice(-2000)));
		bridge.on("error", error => this.drop(new Error(`co-review mcp did not start (${CLI}): ${error.message}`), bridge));
		bridge.on("exit", code => this.drop(new Error(`co-review mcp ended (exit ${code})${stderr.trim() ? `: ${stderr.trim()}` : ""}`), bridge));
		createInterface({ input: bridge.stdout }).on("line", line => this.receive(line));
		await this.request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: this.clientName, version: "1.0.0" } }, 120_000);
		this.write({ jsonrpc: "2.0", method: "notifications/initialized" });
	}

	/** The bridge is gone (or this connection was closed): every waiting call fails, and the next call starts anew. */
	private drop(error: Error, bridge?: ChildProcessWithoutNullStreams): void {
		if (bridge && bridge !== this.bridge) {
			return;
		}
		this.bridge = undefined;
		this.ready = undefined;
		for (const pending of this.pending.values()) {
			pending.reject(error);
		}
		this.pending.clear();
	}

	private write(message: unknown): void {
		this.bridge?.stdin.write(`${JSON.stringify(message)}\n`);
	}

	private receive(line: string): void {
		let message: { id?: number; result?: ToolResult; error?: { message?: string } };
		try {
			message = JSON.parse(line);
		} catch {
			return;
		}
		// Notifications (e.g. Claude Code's channel) are not for this extension: it waits with await_reviewer.
		const pending = typeof message.id === "number" ? this.pending.get(message.id) : undefined;
		if (!pending) {
			return;
		}
		this.pending.delete(message.id as number);
		if (message.error) {
			pending.reject(new Error(message.error.message ?? "Co-Review failed"));
		} else {
			pending.resolve(message.result ?? {});
		}
	}

	private request(method: string, params: unknown, timeoutMs: number, signal?: AbortSignal): Promise<ToolResult> {
		if (!this.bridge) {
			return Promise.reject(new Error("Not connected to Co-Review. Call co_review_start first."));
		}
		const id = ++this.nextId;
		return new Promise<ToolResult>((ok, fail) => {
			const finish = () => {
				clearTimeout(timer);
				signal?.removeEventListener("abort", abort);
			};
			const cancel = (reason: string) => {
				if (this.pending.delete(id)) {
					this.write({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: id, reason } });
					fail(new Error(reason));
				}
				finish();
			};
			const abort = () => cancel("Cancelled");
			const timer = setTimeout(() => cancel(`Co-Review did not answer ${method} in ${Math.round(timeoutMs / 1000)} s`), timeoutMs);
			signal?.addEventListener("abort", abort, { once: true });
			this.pending.set(id, {
				resolve: result => (finish(), ok(result)),
				reject: error => (finish(), fail(error))
			});
			this.write({ jsonrpc: "2.0", id, method, params });
		});
	}

	async call<T = any>(name: string, args: Record<string, unknown>, timeoutMs = 120_000, signal?: AbortSignal): Promise<T> {
		await this.ready;
		const result = await this.request("tools/call", { name, arguments: args }, timeoutMs, signal);
		const text = (result.content ?? []).map(c => c.text ?? "").join("");
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
	async open(root: string, opts: { reviewId?: string; dir?: string; diff?: string; patchName?: string; title?: string; open?: boolean; overview?: string }): Promise<Opened> {
		await this.connect(root);
		return this.call<Opened>("open_review", {
			root, ...(opts.reviewId ? { reviewId: opts.reviewId } : {}), ...(opts.dir ? { dir: resolve(root, opts.dir) } : {}), ...(opts.title ? { title: opts.title } : {}),
			...(opts.diff ? { diff: opts.diff } : {}), ...(opts.patchName ? { patchName: opts.patchName } : {}),
			...(opts.open === false ? { open: false } : {}), ...(opts.overview ? { overview: opts.overview } : {})
		});
	}

	/** Waits for the reviewer: their questions, or their Submit (the verdict); `undefined` when the wait timed out. */
	async next(timeoutSec: number, signal?: AbortSignal): Promise<Verdict | undefined> {
		const r = await this.call<Verdict>("await_reviewer", { timeoutSec }, (timeoutSec + 60) * 1000, signal);
		return r.status === "comment" || r.status === "submitted" || r.status === "closed" ? r : undefined;
	}

	async listen(onThreads: (threads: Thread[]) => void, onVerdict: (verdict: Verdict) => void, onError: (e: Error) => void): Promise<void> {
		if (this.listening) {
			return;
		}
		this.listening = true;
		this.stopped = false;
		while (!this.stopped) {
			try {
				const r = await this.next(240);
				if (r && !this.stopped) {
					if (r.status === "submitted" || r.status === "closed") {
						onVerdict(r);
					} else if (r.threads?.length) {
						onThreads(r.threads);
					}
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
		const bridge = this.bridge;
		this.drop(new Error("Left the review."));
		// Closing stdin ends the bridge's MCP session: Co-Review gives this agent's threads back.
		bridge?.stdin.end();
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
	// The reviewer commented before submitting: those comments come first.
	if (r.status === "comment" && r.threads?.length) {
		return r.threads.map(formatThread).join("\n\n---\n\n");
	}
	if (r.status === "closed") {
		return `[Co-Review] ${ourTools(r.hint ?? "The reviewer closed the review.")}`;
	}
	if (r.status !== "submitted") {
		return "Nothing from the reviewer yet. Call co_review_wait again to keep co-reviewing.";
	}
	const comments = (r.comments ?? []).map(c => `- ${c.id} ${c.where}${c.line ? `:${c.line}` : ""}: ${c.body}`).join("\n");
	const suggestions = (r.acceptedSuggestions ?? []).map(s => `- ${s.path}:${s.startLine}\n  - ${s.before}\n  + ${s.after}`).join("\n");
	return [
		`Round ${r.round}: ${r.decision}${r.summary ? ` — "${r.summary}"` : ""}`,
		comments && `Open comments:\n${comments}`,
		suggestions && `Accepted suggestions (apply each as a commit):\n${suggestions}`
	].filter(Boolean).join("\n\n");
}

/** Co-Review's notes name its MCP tools; this extension's tools have other names. */
function ourTools(note: string): string {
	return note.replace(/\bopen_review\b/g, "co_review_start").replace(/\bawait_reviewer\b/g, "co_review_wait").replace(/\badd_findings\b/g, "co_review_add_findings");
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
			verdict => harness.deliver(ctx, verdict.status === "closed" ? formatVerdict(verdict)
				: `[Co-Review] The reviewer submitted the review. Act on it:\n\n${formatVerdict(verdict)}`),
			error => ctx.ui.setStatus("co-review", `Co-Review: reconnecting (${error.message.slice(0, 60)})`)
		);
	};

	const start = async (ctx: C, opts: { root?: string; reviewId?: string; dir?: string; diff?: string; patchName?: string; title?: string; open?: boolean; overview?: string; listen: boolean }) => {
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
			reviewId: Type.Optional(Type.String({ description: "An existing review to join and show (e.g. one prepared with open: false); the other parameters except open and overview are then ignored" })),
			root: Type.Optional(Type.String({ description: "Absolute repository path; defaults to the working directory" })),
			dir: Type.Optional(Type.String({ description: "Review directory with a design document (index.markdown) and/or *.patch files, relative to root or absolute" })),
			diff: Type.Optional(Type.String({ description: "A git revision range to review as a pull-request page, diffed by Co-Review: \"HEAD\" (uncommitted, new files included) or \"main...HEAD\"; with dir, written into it next to a PR.md" })),
			patchName: Type.Optional(Type.String({ description: "Slug of the diff's page (default \"change\"); findings on it use target \"patch:<slug>\"" })),
			title: Type.Optional(Type.String({ description: "Title for a new review; omit to join the latest one" })),
			open: Type.Optional(Type.Boolean({ description: "Show the review to the reviewer (default true); false prepares it without showing it, e.g. while you add first-pass findings — call again to show it" })),
			overview: Type.Optional(Type.String({ description: "Your overview for the review's front page (Markdown), which the reviewer reads first: what it does and why, how it works as a ```mermaid flowchart, what else it affects, where to start. Pass it with reviewId to add or update it" }))
		}),
		async execute(_id: string, params: { root?: string; reviewId?: string; dir?: string; diff?: string; patchName?: string; title?: string; open?: boolean; overview?: string }, _signal: AbortSignal, _onUpdate: unknown, ctx: C) {
			const listen = ctx.hasUI && harness.isMain(ctx);
			const opened = await start(ctx, { ...params, listen });
			const warnings = opened.format?.warnings?.length ? `\nFix the design document, then call co_review_start again: ${opened.format.warnings.join(" ")}` : "";
			const where = ourTools((opened.url ? `Reviewer URL: ${opened.url}` : opened.note ?? "") + warnings);
			return text(listen
				? `Joined review ${opened.reviewId} (${opened.openThreads} open threads). ${where}\nReviewer questions will arrive as [Co-Review] messages; answer each with co_review_reply.`
				: `Joined review ${opened.reviewId} (${opened.openThreads} open threads). ${where}\nNow loop: co_review_wait → investigate → co_review_reply, until the reviewer says the review is done.`);
		}
	});

	pi.registerTool({
		name: "co_review_wait",
		label: "Co-Review: wait",
		description: "Wait for the reviewer's next move (the only call to wait on): their questions and answers to reply to, or their Submit (the decision, open comments and accepted suggestions) to act on.",
		parameters: Type.Object({
			timeoutSec: Type.Optional(Type.Number({ description: "Seconds to wait (default 600)" }))
		}),
		async execute(_id: string, params: { timeoutSec?: number }, signal: AbortSignal) {
			const r = await connection.next(params.timeoutSec ?? 600, signal);
			return text(r?.status === "submitted" ? `The reviewer submitted the review. Act on it:\n\n${formatVerdict(r)}` : formatVerdict(r ?? { status: "pending" }));
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
		description: "Add review findings as threads. On repository code: path (lines are 1-based; without a line, on the file or folder). On a patch page or a Markdown page of a review directory: target (\"patch:<slug>\", \"doc\", \"doc:<path>\") and anchor (e.g. { type: \"code-line\", path, line, side: \"new\" } on a patch); those arrive proposed. labels group them in the panel (e.g. the area); status \"proposed\" lets the reviewer accept or dismiss each.",
		parameters: Type.Object({
			findings: Type.Array(Type.Object({
				path: Type.Optional(Type.String({ description: "File or folder path relative to the repository (\".\" for the repository)" })),
				target: Type.Optional(Type.String({ description: "\"patch:<slug>\", \"doc\" or \"doc:<path>\" (with anchor)" })),
				anchor: Type.Optional(Type.Record(Type.String(), Type.Any(), { description: "Where on the page, e.g. { type: \"code-line\", path, line, side: \"new\" } or { type: \"text\", exact }" })),
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
		name: "co_review_post_github",
		label: "Co-Review: post to GitHub",
		description: "Post the review to its GitHub pull request (a review directory whose PR.md names it) as one GitHub review: line comments, suggestions, and the reviewer's verdict. Only when the reviewer asks: Co-Review shows them what will be posted and posts only if they confirm.",
		parameters: Type.Object({
			summary: Type.Optional(Type.String({ description: "The review's message on GitHub (default: the reviewer's summary)" }))
		}),
		async execute(_id: string, params: { summary?: string }) {
			const r = await connection.call<{ status: string; url?: string; comments?: number; inBody?: number; notes?: string[] }>("post_review_to_github", params, 16 * 60_000);
			return text(r.status === "posted"
				? `Posted to GitHub: ${r.url} (${r.comments} line comments${r.inBody ? `, ${r.inBody} in the body` : ""}).${r.notes?.length ? ` ${r.notes.join(" ")}` : ""}`
				: "The reviewer didn't confirm; nothing was posted.");
		}
	});

	pi.registerTool({
		name: "co_review_ask",
		label: "Co-Review: ask reviewer",
		description: "Ask the reviewer to choose between options (shown as buttons in the review), then wait like co_review_wait: returns their choice, or whatever they did first (the choice then comes from co_review_wait).",
		parameters: Type.Object({
			question: Type.String(),
			options: Type.Array(Type.String(), { minItems: 1, maxItems: 6 }),
			threadId: Type.Optional(Type.String())
		}),
		async execute(_id: string, params: Record<string, unknown>, signal: AbortSignal) {
			const r = await connection.call<Verdict & { question: { threadId: string; choice?: string } }>("ask_reviewer", params, 960_000, signal);
			const answer = r.question.choice !== undefined ? `The reviewer chose: ${r.question.choice}`
				: `The reviewer has not chosen yet (thread ${r.question.threadId}); their choice comes from co_review_wait.`;
			return text(r.status === "pending" ? answer : `${answer}\n\n${r.status === "submitted" ? `The reviewer submitted the review. Act on it:\n\n${formatVerdict(r)}` : formatVerdict(r)}`);
		}
	});
}
