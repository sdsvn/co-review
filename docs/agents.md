# Agents in a review

An agent is a **participant** in a review: it answers questions in threads, adds findings, and
asks the reviewer to decide. There are two ways for an agent to join, depending on *who starts whom*.

| | **ACP** — Co-Review starts the agent | **MCP / Pi** — the agent (in its harness) starts Co-Review |
|---|---|---|
| Who initiates | The reviewer, from the Review panel (*Connect agent*) | The agent: Claude Code or Pi working on a task |
| Transport | [Agent Client Protocol](https://agentclientprotocol.com) over stdio | [Model Context Protocol](https://modelcontextprotocol.io) tools (Streamable HTTP, or stdio via `co-review mcp`) |
| Latency | Immediate: every question starts a turn; the answer **streams** | The agent answers when it calls `await_comment`; answers arrive whole |
| Agent's context | A fresh session per thread (knows only the repo + thread) | The agent's own session — it knows what it just built and why |
| Agent activity | Tool calls shown live; permission requests as buttons | `ask_reviewer` for decisions; the rest happens in the harness |
| Agents | Claude Code (`claude-agent-acp`), Gemini CLI, OpenCode, Goose, any ACP agent | Claude Code (plugin), Pi and Oh My Pi (native packages), Codex, Cursor, VS Code, Zed, any MCP client |

**Rule of thumb:** use **ACP** when you start reviewing and want an on-call expert (fast, streaming).
Use **MCP** when an agent did (or is doing) the work and should defend and explain it as your
co-reviewer — only that agent has the context.

Both land in the same threads, inline in the editor and in the Review panel.

## How agents answer

Every agent follows the same instruction: the reviewer is a person reading a thread, so answer the question first, in plain language, in a few sentences. Explain in words rather than walking through paths and line numbers. One or two links at the end are fine when they help.

Agents read only what the answer needs.

### Repository map

To make answers fast, Co-Review keeps a **repository map** — a listing of every source file with the classes, functions and methods it defines, built from Tree-sitter.

The map is:

- Built on demand (when an agent connects, or on the first `repo_map` call).
- Stored next to the reviews in `~/.co-review`.
- Refreshed for files that changed since the last build.

An ACP agent gets the part of the map nearest the question in its first prompt. MCP agents call `repo_map` (Pi: `co_review_map`). **Graphify.** When the `graphify` command ([Graphify](https://graphify.net), `uv tool install graphifyy` or `pipx install graphifyy`) is installed, opening the repository overview (**Review: Open Repository Overview**, or `repo_map({ overview: true })` for agents) first builds or refreshes the repository's Graphify graph with `graphify update`. For code that is Tree-sitter only (no LLM, no API key) and takes seconds. The overview is then built from the graph: the most connected code, its clusters, and a diagram of how they connect, with Graphify's `GRAPH_REPORT.md` (import cycles, surprising connections) linked from it. The graph lives in `graphify-out/` in the repository; if Co-Review created that folder, it adds it to the clone's `.git/info/exclude`, so it never shows up as a change. Agents can also ask the graph directly: `graphify query`, `graphify path`, `graphify explain`.

## ACP: connect an agent from Co-Review

ACP (Agent Client Protocol) lets the reviewer summon an agent on demand from the Review panel.

1. Open the Review panel → **Connect agent** → pick a detected agent (Claude Code, Gemini CLI, OpenCode, Goose, Oh My Pi,
   Pi, Codex — whichever are installed) or enter any ACP command.
   Then pick the **model** and **reasoning effort**, from the list the agent itself offers (Escape keeps its default).
   The choice shows next to the agent in the panel; click it to change it.
2. Select code → **Ask Agent** (or click **+** in the gutter and drag over several lines — asking is the default for multi-line comments).
3. The answer streams into the thread. Reply in the thread to follow up (same ACP session).
   Permission requests appear as buttons; **Stop** cancels the turn.

The agent runs in the repository root, with the environment of the Co-Review process (its normal login/config). Its file reads and writes through ACP are limited to the repository.

## MCP: let an agent in a harness open a review

MCP (Model Context Protocol) lets an agent that is already running in its own harness join a review.

The setup depends on the harness:

- **Claude Code** — install the Co-Review plugin. It provides tools, skills, slash commands, a background `co-reviewer` subagent, a session hook, and an optional live channel that pushes your questions into the session.
- **Pi** — install the Co-Review package.
- **Oh My Pi** — install the Co-Review omp package.
- **Other harnesses** (Codex, Cursor, VS Code, Zed, Gemini CLI, OpenCode, Goose, etc.) — add the MCP server and, optionally, the skills.

[Connect your agent](agent-setup.md) has the config for each harness. Inside Co-Review, **Review: Add Co-Review to an Agent Harness (MCP)…** does it for you.

### How `co-review mcp` works

`co-review mcp` uses the harness's working directory as the repository and bridges stdio to the `/mcp` endpoint of a running Co-Review (desktop or browser). When none is running, it answers the agent's handshake itself and starts Co-Review on the first tool call, so opening an agent session doesn't start the app.

A client that speaks Streamable HTTP can also connect directly (local connections only):

```
http://127.0.0.1:<port>/mcp?root=<repo>
```

### Starting a review

Pick the [workflow](workflows.md): `/co-review:audit` (the whole repository), `/co-review:review` (the agent's
change), `/co-review:pr <n>` (someone else's pull request) or `/co-review:design <task>` in Claude Code; the same with
`/co-review-audit`, `/co-review-change`, `/co-review-pr` and `/co-review-design` in Pi or Oh My Pi. In any other
harness, say what you want, e.g.:

> Review the change you just made with me in Co-Review: add findings for anything risky, then keep answering my
> questions in the review until I submit.

### The agent loop

```
open_review({ …, open: false })   prepare it, without showing it
add_findings                      the first pass
open_review({ reviewId })         show it
loop:
  await_comment                   (blocks until a question arrives)
  … investigate …
  reply(threadId, body)
await_review                      the verdict, then act on it
```

While the agent is blocked in `await_comment`, the agent next to the review's scope in the panel is marked as listening (hover it for the status). When it is busy elsewhere, your questions wait and are delivered on its next `await_comment`.

The full tool contract is in [`../llms.txt`](../llms.txt).

## Use it in your workflow

Skills, the Claude Code plugin and Pi: see [Connect your agent](agent-setup.md).

## Review directories

An agent can hand over more than code: a directory with a design document and patches. Open it with `open_review({ dir })` (or `({ markdown, patch })` for inline content, or `({ diff })` to have Co-Review diff the repository itself; `({ dir, diff })` writes that diff into the directory, next to a pull request's `PR.md`). The format is in [`../llms.txt`](../llms.txt).

- **Rendering** — the document (Markdown, Mermaid, the L1/L2/L3 design tree; see [design-docs.md](design-docs.md)) and each `*.patch` as a review page with PR metadata. Findings in `<patch>.comments.json[l]` appear as *proposed* (Accept / Dismiss).
- **Verdict** — the reviewer submits with **Submit review** (Approve / Request changes / Comment + a message). `await_review` returns it with the open comments.
- **Suggested edits** — findings may carry `proposal: { before, after, path?, startLine? }`, and the reviewer can propose edits on the document. Accepted document edits are written into the document (`doc.version` increments). Accepted patch suggestions come back in `acceptedSuggestions` for the agent to commit on the branch.
- **OpenSpec** — `<dir>/openspec` (or `open_review({ openspec })`) renders as cards under the document.
- **State file** — the review is also written to `<dir>/review.json` (or `storePath`), for tools that read it from disk.
- **Server mode** — `co-review-server` opens a directory for review from a script and serves a small HTTP API. See [`../bin/review-server.mjs`](../bin/review-server.mjs).
  ```
  co-review-server -dir <dir> -store <file>
  ```
- **Phone** — `/m/` (see [running-and-packaging.md](running-and-packaging.md#phone-view)).
