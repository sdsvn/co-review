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
| Agents | Claude Code (`claude-agent-acp`), Gemini CLI, OpenCode, Goose, any ACP agent | Claude Code (plugin), Pi (native package), Codex, Cursor, VS Code, Zed, any MCP client |

**Rule of thumb:** use **ACP** when you start reviewing and want an on-call expert (fast, streaming).
Use **MCP** when an agent did (or is doing) the work and should defend and explain it as your
co-reviewer — only that agent has the context.

Both land in the same threads, inline in the editor and in the Review panel.

## ACP: connect an agent from Co-Review

1. Open the Review panel → **Connect agent** → pick a detected agent or enter any ACP command.
2. Select code → **Ask Agent** (or click **+** in the gutter and drag over several lines — asking is the default for multi-line comments).
3. The answer streams into the thread. Reply in the thread to follow up (same ACP session).
   Permission requests appear as buttons; **Stop** cancels the turn.

The agent runs in the repository root, with the environment of the Co-Review process (its normal
login/config). Its file reads/writes through ACP are limited to the repository.

## MCP: let an agent in a harness open a review

In **Claude Code**, install the Co-Review plugin: tools, skills, slash commands, a background `co-reviewer` subagent, a
session hook and an optional live channel that pushes your questions into the session. In **Pi**, the Co-Review
package. Other harnesses (Codex, Cursor, VS Code, Zed, Gemini CLI, OpenCode, Goose, …) add the MCP
server and, optionally, the skills. [Connect your agent](agent-setup.md) has the config for each harness; inside
Co-Review, **Review: Add Co-Review to an Agent Harness (MCP)…** does it for you.

`co-review mcp` uses the harness's working directory as the repository, reuses a running Co-Review
(desktop or browser) or starts the browser app, and bridges stdio to its `/mcp` endpoint. A client
that speaks Streamable HTTP can also connect directly to `http://127.0.0.1:<port>/mcp?root=<repo>`
(local connections only).

Then start it: `/co-review:review` in Claude Code, `/co-review` in Pi, or say it in any harness:

> Open a Co-Review review for this repository and be my co-reviewer: add findings for anything
> risky in your change, then keep answering my questions in the review until I say we're done.

The agent loop:

```
open_review            → returns the URL (browser) and opens it; the desktop app switches to it
add_findings (optional)
loop:
  await_comment        → blocks until you ask something or reply in a thread it is part of
  … investigate …
  reply(threadId, body)
```

While the agent is blocked in `await_comment` the panel shows **listening**; when it is busy
elsewhere, your questions wait and are delivered on its next `await_comment`.

The full tool contract is in [`../llms.txt`](../llms.txt).

## Use it in your workflow

Skills, the Claude Code plugin and Pi: see [Connect your agent](agent-setup.md).

## Review directories

An agent can hand over more than code: a directory with a design document and patches, opened with
`open_review({ dir })` (or `({ markdown, patch })` for inline content). The format is in [`../llms.txt`](../llms.txt).

- **Rendering**: the document (Markdown, Mermaid, the L1/L2/L3 design tree; see [design-docs.md](design-docs.md)) and
  each `*.patch` as a review page with PR metadata. `<patch>.comments.json[l]` findings appear as *proposed*
  (Accept / Dismiss).
- **Verdict**: the reviewer submits with **Submit review** (Approve / Request changes / Comment + a message);
  `await_review` returns it with the open comments.
- **Suggested edits**: findings may carry `proposal: { before, after, path?, startLine? }`, and the reviewer can
  propose edits on the document. Accepted document edits are written into the document (`doc.version` increments);
  accepted patch suggestions come back in `acceptedSuggestions` for the agent to commit on the branch.
- **OpenSpec**: `<dir>/openspec` (or `open_review({ openspec })`) renders as cards under the document.
- **State file**: the review is also written to `<dir>/review.json` (or `storePath`), for tools that read it from disk.
- **Server mode**: `co-review-server -dir <dir> -store <file>` (installed next to `co-review`) opens a directory for
  review from a script and serves a small HTTP API; see [`../bin/review-server.mjs`](../bin/review-server.mjs).
- **Phone**: `/m/` (see [running-and-packaging.md](running-and-packaging.md#phone-view)).
