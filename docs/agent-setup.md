# Connect your agent

Co-Review is built for **Claude Code**: one plugin install brings the review tools, skills, slash commands, a
background co-reviewer, live review comments and a session hook. **Pi** gets a native package too, and any other MCP
harness (Codex, Cursor, VS Code, Gemini CLI, Zed, …) works through the same MCP server.

What an agent gets:

- **The review tools** (MCP): `open_review`, `await_comment`, `reply`, `add_findings`, `ask_reviewer`, `await_review`,
  `get_review`.
- **Two skills.** `co-review` hands a change to you, answers your comments in their threads and acts on your
  verdict. `co-review-design` writes a design document, has you review it, and implements only after you approve.
- **Slash commands** to start either workflow.

**Before you start:** agents launch Co-Review through the `co-review` command, so it has to be on your `PATH`.
Check with `which co-review`.

- If you installed with `install.sh`, it's already there.
- If you built from source, run `make install-cli`.
- Or, inside Co-Review, open the command palette (`Cmd/Ctrl+Shift+P`) and run **Review: Install the co-review
  Command**.

## Claude Code

Co-Review is built for Claude Code. The Co-Review plugin brings everything in one install.

**Option 1: from Claude Code.** Type these two commands in a Claude Code session:

```bash
/plugin marketplace add sdsvn/co-review
/plugin install co-review@co-review
```

**Option 2: let Co-Review do it.** In Co-Review, open the Review panel and click **Connect an Agent** (or run
**Review: Add Co-Review to an Agent Harness (MCP)…** from the command palette). Pick **Claude Code: plugin** and
confirm. Co-Review runs the same two steps with the `claude` command line.

Either way, restart Claude Code afterwards so it loads the plugin.

| The plugin adds | What it does |
|---|---|
| **MCP server** `plugin:co-review:co-review` | The review tools: `open_review`, `await_comment`, `reply`, `add_findings`, `ask_reviewer`, `await_review`, `get_review` |
| **Skills** `co-review`, `co-review-design` | The workflows; they also trigger on requests like "review this with me" or "design this first" |
| **`/co-review:review [focus]`** | Opens this change in Co-Review and stays as your co-reviewer until you submit |
| **`/co-review:design <task>`** | Writes a design doc, opens it for review, revises it, implements after approval |
| **`/co-review:audit [focus]`** | Reviews the whole repository with you: first-pass findings grouped by area, then answers your questions |
| **Subagent** `co-review:co-reviewer` | Runs in the background and answers your review while the main conversation keeps working ("keep answering my review while you fix the tests") |
| **Live channel** | Your questions and your Submit arrive in the running session the moment you make them (below) |
| **SessionStart hook** | Opening Claude Code in a repository with open reviews tells it which questions are waiting |

Check it with `/mcp`: `plugin:co-review:co-review` should be connected. The plugin launches `co-review mcp`, so the
`co-review` command has to be installed ([Install](../README.md#install)).

### Live review comments (channel)

By default Claude Code waits for your comments with `await_comment`, which works everywhere. With Claude Code's
[channels](https://code.claude.com/docs/en/channels), Co-Review pushes each question into the conversation as you
ask it, and Claude answers in the thread and carries on with its work. Nothing blocks and no turn is spent waiting.

Channels are a research preview, and a plugin outside Anthropic's allowlist has to be loaded explicitly:

```bash
claude --dangerously-load-development-channels plugin:co-review@co-review
```

Claude Code asks you to confirm once. Channels need a claude.ai or Console login (not Bedrock or Vertex), and Team
or Enterprise admins have to enable them (`channelsEnabled`). Without the flag, the plugin still works through
`await_comment`, and nothing is lost: a pushed question is still returned by `await_comment` if the session didn't
receive it.

### The hook

At session start the plugin asks a running Co-Review about open reviews of the project (`hooks/session-start.sh`,
using `curl`). If questions are waiting, Claude Code is told, so "let's continue the review" just works. The hook
never starts Co-Review, and prints nothing when there is nothing to say.

### Without the plugin

MCP server only (no skills, commands, subagent, channel or hook):

```bash
claude mcp add -s user co-review -- co-review mcp
```

For a team, commit it to the project's `.mcp.json`:

```json
{ "mcpServers": { "co-review": { "command": "co-review", "args": ["mcp"] } } }
```

### Claude Code inside Co-Review

Claude Code can also be the agent Co-Review launches for a question (ACP): **⋯ → Connect agent… → Claude Code** in
the Review panel, then select code → **Ask Agent**. The answer streams into the thread. See
[agents.md](agents.md) for when to use which.

## Pi

The Pi package is native: it talks to Co-Review directly, so Pi doesn't need MCP support. The package ships with
Co-Review, so you install it from where Co-Review is installed.

**Option 1: let Co-Review do it.** In Co-Review, open the Review panel and click **Connect an Agent** (or run
**Review: Add Co-Review to an Agent Harness (MCP)…** from the command palette, `Cmd/Ctrl+Shift+P`). Pick
**Pi: package** and confirm. Co-Review runs `pi install` with the right path for you.

**Option 2: install it from a terminal.** Run the command for how you installed Co-Review:

```bash
# macOS app (installed by install.sh)
pi install /Applications/Co-Review.app/Contents/Resources/app/integrations/pi
```

```bash
# Linux (installed by install.sh)
pi install ~/.local/share/co-review/resources/app/integrations/pi
```

```bash
# From a clone of the repository
pi install ./integrations/pi
```

On macOS, use `~/Applications/Co-Review.app/…` instead if the app was installed there (`install.sh` falls back to
it when `/Applications` isn't writable). Then start a new Pi session so it loads the package.

| In Pi | Does |
|---|---|
| `/co-review` | Opens a review of the working directory; your questions arrive in the session as you ask them |
| `/co-review-design <task>` | Writes a design doc, opens it for review, implements after approval |
| `/co-review-audit [focus]` | Reviews the whole repository with you: first-pass findings grouped by area |
| `pi --co-review` | Starts Pi already listening to the review |
| `co_review_start`, `co_review_wait`, `co_review_reply`, `co_review_add_findings`, `co_review_ask`, `co_review_map` | The tools, for the model and for subagents |
| `co-reviewer` subagent | Stays in the review as co-reviewer while the main session keeps working |

The package also loads the `co-review` and `co-review-design` skills. In an interactive session, Pi waits for your
questions without spending tokens: a background listener hands each one to the session.

## Skills for other agents

| Where | Install |
|---|---|
| Any agent, with the [`skills`](https://www.npmjs.com/package/skills) CLI | `npx skills add sdsvn/co-review` |
| From inside Co-Review | Command palette → **Review: Install Agent Skills…**, then pick Claude Code, `~/.agents/skills`, or this repository |
| Manually | copy `plugin/skills/co-review` and `plugin/skills/co-review-design` into the agent's skills directory |

The skills live in [`plugin/skills`](../plugin/skills). The Claude Code plugin is [`plugin/`](../plugin), listed in
[`.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json).

## Other harnesses (MCP)

Every harness launches the same stdio command, `co-review mcp`. It finds the running Co-Review (desktop or
browser) through `~/.co-review/server.json`, or starts it, and uses the harness's working directory as the
repository.

> [!NOTE]
> Apps opened from the Dock (Cursor, Claude Desktop, VS Code) may not have `~/.local/bin` on their `PATH`. Use the
> absolute path instead, for example `/usr/local/bin/co-review` or `~/.local/bin/co-review` (`which co-review`
> shows it).
>
> Or let Co-Review write the config: **Connect an Agent** in the Review panel, then pick the harness. It merges JSON
> configs (keeping a backup), appends to TOML ones, and copies the snippet to paste where a config can't be edited
> safely (Zed, Goose).

> [!TIP]
> `await_comment` and `await_review` wait for you for up to a few minutes. Where a harness has a tool-call timeout,
> raise it to 600 s, as the examples below do.

### Codex

`~/.codex/config.toml`:

```toml
[mcp_servers.co-review]
command = "co-review"
args = ["mcp"]
tool_timeout_sec = 600
```

### Cursor

`~/.cursor/mcp.json` (or `.cursor/mcp.json` in a project):

```json
{ "mcpServers": { "co-review": { "command": "co-review", "args": ["mcp"] } } }
```

### Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{ "mcpServers": { "co-review": { "command": "/usr/local/bin/co-review", "args": ["mcp"] } } }
```

### VS Code (Copilot agent mode)

User `mcp.json` (**MCP: Open User Configuration**) or `.vscode/mcp.json` in a project:

```json
{ "servers": { "co-review": { "type": "stdio", "command": "co-review", "args": ["mcp"] } } }
```

### Windsurf

`~/.codeium/windsurf/mcp_config.json`:

```json
{ "mcpServers": { "co-review": { "command": "co-review", "args": ["mcp"] } } }
```

### Gemini CLI

`~/.gemini/settings.json`:

```json
{ "mcpServers": { "co-review": { "command": "co-review", "args": ["mcp"], "timeout": 600000 } } }
```

### OpenCode

`~/.config/opencode/opencode.json`:

```json
{ "mcp": { "co-review": { "type": "local", "command": ["co-review", "mcp"], "enabled": true } } }
```

### Zed

`settings.json` (**zed: open settings**):

```json
{ "context_servers": { "co-review": { "source": "custom", "command": "co-review", "args": ["mcp"] } } }
```

### Goose

`goose configure` → *Add Extension* → *Command-line Extension* with the command `co-review mcp`, or in
`~/.config/goose/config.yaml`:

```yaml
extensions:
  co-review:
    name: co-review
    type: stdio
    cmd: co-review
    args: [mcp]
    enabled: true
    timeout: 600
```

### Anything else

Any MCP client that runs stdio servers: command `co-review`, arguments `mcp`. Clients that speak Streamable HTTP
can connect to `http://127.0.0.1:<port>/mcp?root=<absolute repo path>` instead (local connections only; the port
is in `~/.co-review/server.json`).

## Without skills

For an agent that doesn't load skills, add to its `AGENTS.md` / `CLAUDE.md`:

> When a change is ready, or a design needs review, open it in Co-Review with the `open_review` tool and stay as
> co-reviewer: answer every comment in its thread, and wait for my verdict (`await_review`) before committing.
