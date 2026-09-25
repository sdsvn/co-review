# Connect your agent

Two things make an agent a co-reviewer:

- **Co-Review's MCP server** gives it the review tools (`open_review`, `await_comment`, `reply`, …).
- **The skills** teach it the workflow: `co-review` hands a change to you and answers your comments, and
  `co-review-design` writes a design doc, has it reviewed and implements after approval.

The quickest way is from inside Co-Review. Open the Command Palette (`Cmd+Shift+P`) or the Review panel's **⋯**:

- **Review: Add Co-Review to an Agent Harness (MCP)…** picks a harness and adds the server to its config.
  JSON files are merged (a backup is kept) and TOML is appended. For formats that can't be edited safely (Zed,
  Goose) it copies the snippet for you to paste. The command it writes launches Co-Review with this machine's
  paths, so it works even when the harness is opened from the Dock.
- **Review: Install Agent Skills…** copies both skills into Claude Code's skills directory, the shared
  `~/.agents/skills` (Pi and the `skills` CLI), or this repository's `.claude/skills`.

Everything below is the same, done by hand.

## Skills

| Where | Install |
|---|---|
| Claude Code (plugin: skills **and** MCP server) | `/plugin marketplace add sdsvn/co-review`, then `/plugin install co-review@co-review` |
| Any agent, via the [`skills`](https://www.npmjs.com/package/skills) CLI | `npx skills add sdsvn/co-review` |
| Pi | `npx skills add sdsvn/co-review -g` (Pi reads `~/.agents/skills`), or the native package below |
| From a checkout | point either installer at the path instead: `npx skills add ./co-review`, `/plugin marketplace add ./co-review` |
| Manually | copy `plugin/skills/co-review` and `plugin/skills/co-review-design` into your agent's skills directory |

The skills live in [`plugin/skills`](../plugin/skills). The Claude Code plugin is [`plugin/`](../plugin), listed by
the marketplace in [`.claude-plugin/marketplace.json`](../.claude-plugin/marketplace.json). The plugin also registers
the MCP server, so it needs the `co-review` command on your `PATH` (see
[Install](../README.md#install)).

Pi also has a native package with a `/co-review` command, a background listener and a `co-reviewer` subagent:
`pi install <path to co-review>/integrations/pi`.

## MCP server

Every harness launches the same stdio command, `co-review mcp`. It finds the running Co-Review (desktop or
browser) through `~/.co-review/server.json`, or starts it, and uses the harness's working directory as the
repository.

> [!NOTE]
> Apps opened from the Dock (Cursor, Claude Desktop, VS Code) may not have `~/.local/bin` on their `PATH`. Use the
> absolute path, for example `/usr/local/bin/co-review` or `~/.local/bin/co-review` (`which co-review` shows it), or
> let **Add Co-Review to an Agent Harness** write it for you.

> [!TIP]
> `await_comment` and `await_review` wait for you for up to a few minutes. Where a harness has a tool-call timeout,
> raise it to 600 s, as the examples below do.

### Claude Code

```bash
claude mcp add -s user co-review -- co-review mcp
```

Or share it with a project in `.mcp.json`:

```json
{ "mcpServers": { "co-review": { "command": "co-review", "args": ["mcp"] } } }
```

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
