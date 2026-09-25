<div align="center">

<img src="docs/assets/logo.svg" width="88" alt="Co-Review logo" />

# Co-Review

**Review code and designs with an AI agent as your co-reviewer.**

[![Built on Eclipse Theia](https://img.shields.io/badge/built%20on-Eclipse%20Theia-4F46E5?style=flat-square)](https://theia-ide.org)
[![Agents](https://img.shields.io/badge/agents-ACP%20%2B%20MCP-059669?style=flat-square)](docs/agents.md)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-1E1B4B?style=flat-square)](#install)

[Install](#install) · [Usage](#usage) · [Agents](#working-with-agents) · [Design docs](docs/design-docs.md) · [Contributing](#contributing)

</div>

Co-Review is a review tool with IDE features, not an IDE. Open a repository, a branch, a commit or a design document,
comment on it the way you would on a pull request, and ask an agent about anything you're reading. The agent answers
in the same thread, with links into the code, and waits for your verdict before it changes anything.

It is built on [Eclipse Theia](https://theia-ide.org) as a platform, so code navigation, search, Git and language
servers are the real thing. Everything unrelated to reviewing is removed.

```
 you ── inline threads, Ask Agent, Submit review ──┐
                                                   ▼
                     Co-Review  (browser or desktop app, Theia)
                      │  reviews in ~/.co-review, outside the repo
          ┌───────────┴────────────┐
         ACP                      MCP
 Co-Review launches the    the agent that did the work opens the
 agent for your question   review from its own harness and answers
 (Claude Code, Gemini,     your comments in a loop
  OpenCode, Goose)         (Claude Code, Codex, Cursor, Pi, …)
```

## Features

- **Inline threads.** Comment on a line, a range, a symbol, a file, a folder or the whole repository. Threads sit
  under the code, and drafts stay open while you read elsewhere.
- **Comments that follow the code.** Each comment is anchored to its symbol and its Tree-sitter tokens. It survives
  edits and moves, and it's marked *outdated* rather than jumping onto code it wasn't about.
- **Agents as reviewers.** Select lines and **Ask Agent**. The answer streams into the thread with the agent's steps,
  and permission requests become buttons for you.
- **Design review.** Rendered Markdown, Mermaid diagrams with per-node comments, and a foldable L1 · L2 · L3 design
  tree whose structure is [declared and checked](docs/design-docs.md).
- **Patch review.** `.patch` and `.diff` files open as pull-request pages with PR metadata and line comments.
- **Findings and suggestions.** Agent findings arrive as *proposed*, and you accept or dismiss them. Suggested edits
  are accepted with one click.
- **One verdict.** Approve, Request changes or Comment, with a message. The agent gets every open comment in one batch.
- **Phone view.** An installable page to read, reply, accept findings and submit away from your desk.
- **Desktop app and CLI.** `Co-Review.app` with a `co-review` command, or the browser app from a checkout.

## Why not pull-request review?

Pull requests review a diff after the fact, and a bot's comments land next to yours with no way to ask it a
follow-up. Co-Review reviews the whole repository, including code the diff doesn't touch, reviews designs before
there is code, and the agent that did the work answers where you asked.

# Using Co-Review

## Install

### macOS: app and command line

```bash
make install        # dependencies and bundled VS Code extensions (first time)
make install-app    # Co-Review.app into /Applications, plus the `co-review` command
```

Or install from the disk image built by `make package` (`applications/electron/dist/Co-Review-<version>-arm64.dmg`).
Drag the app to Applications, then add the command:

```bash
/Applications/Co-Review.app/Contents/Resources/app/bin/install-cli.sh
```

> [!NOTE]
> Local builds are unsigned: the first time, open the app with right-click → **Open**.

```bash
co-review ~/src/my-service     # open a repository
co-review mcp                  # MCP server for agent harnesses; starts the app if needed
```

### From source (any OS)

Requires Node.js 20+, Git and a C/C++ toolchain (Xcode command-line tools on macOS, `build-essential` on Linux).

```bash
make install
make start REPO=/path/to/repo      # browser app on http://127.0.0.1:3000
make desktop REPO=/path/to/repo    # desktop app
make install-cli                   # `co-review` command running from this checkout
```

## Usage

### Start a review

Open the **Review** panel from the status bar or with `Cmd+Shift+Alt+R`, and pick what to review: the **entire
repository**, the **current file**, a **branch** against its base, or a **commit**. You can also right-click files
or folders → **Start Review of Selection**.

### Comment and discuss

| To comment on | Do this |
|---|---|
| A line | Hover it and click **+** in the gutter |
| Several lines | Drag the **+**, or select them and press `Cmd+Alt+M` |
| A symbol | Right-click inside it → **Add Review Comment on Symbol** |
| A file or folder | Right-click it in the explorer → **Add Review Comment** |
| The repository | **⋯** in the Review panel → **Comment on the repository** |

Until an agent joins the review, everything is a plain comment: **Ask Agent** only appears once one is connected
(**⋯ → Connect agent…**, or an agent opens the review over MCP). Then a multi-line selection defaults to asking.

Reply, resolve and collapse threads inline. The panel lists threads by file (**Open / Proposed / Resolved / All**).
**⋯** has the rest: go to a comment, collapse or expand all, change agent, open the phone view, rename or delete.

### Designs and patches

`index.markdown` and `*.pseudocode.md` open in the rendered review view. For any other `.md` file, use **Open With
→ Review (rendered)**. From there you can:

- select text to comment, ask or **Propose Edit**;
- comment on a diagram, or on one of its nodes or edges;
- comment on any step of the design tree.

`*.patch` and `*.diff` files open as pull-request pages.

> [!TIP]
> Have agents write design documents with `co-review: design` frontmatter. Co-Review checks the structure and tells
> both of you about any mismatch. See [Design documents](docs/design-docs.md).

### From your phone

**⋯ → Open mobile view**, or `http://<host>:<port>/m/`. It accepts local connections only. To use it over Tailscale,
run `tailscale serve --bg 3000` and start Co-Review with `CO_REVIEW_ALLOWED_HOSTS=<your-mac>.<tailnet>.ts.net`.

## Working with agents

| | ACP: Co-Review launches the agent | MCP: the agent brings Co-Review |
|---|---|---|
| Best for | Asking questions while you review | An agent that did the work and hands it to you |
| Setup | **⋯ → Connect agent…** | `claude mcp add -s user co-review -- co-review mcp` |
| Flow | Select code → **Ask Agent** (`Cmd+Alt+A`) | Agent calls `open_review`, then answers in a loop |

With MCP, tell your agent something like:

> Open a Co-Review review of this change, add findings for anything risky, then answer my questions in the review
> until I submit.

The panel shows the agent as **listening**, or **busy** while it works; your questions are delivered when it checks
in. The full contract is in [llms.txt](llms.txt), and [integrations/pi](integrations/pi) is a native
[Pi](https://pi.dev) package with a `/co-review` command and a `co-reviewer` subagent.

To set up an agent, open the Command Palette in Co-Review and run **Review: Add Co-Review to an Agent Harness
(MCP)…** and **Review: Install Agent Skills…**. Or install from your agent:

```bash
/plugin marketplace add sdsvn/co-review     # Claude Code: skills + MCP server
/plugin install co-review@co-review
npx skills add sdsvn/co-review              # any agent: the co-review and co-review-design skills
```

[Connect your agent](docs/agent-setup.md) has the MCP config for Claude Code, Codex, Cursor, Claude Desktop, VS Code,
Windsurf, Gemini CLI, OpenCode, Zed and Goose, plus Pi.

> [!IMPORTANT]
> Agents and language servers use Co-Review's environment. The desktop app loads your login shell's `PATH`, and
> `co-review` passes on the terminal's.

## Language support

Navigation (go to definition, references, hover) comes from VS Code extensions bundled in `plugins/`:

| Language | Provided by | Needs |
|---|---|---|
| TypeScript, JavaScript, JSON, Markdown, PHP | VS Code built-ins | — |
| Go | [`golang.Go`](https://open-vsx.org/extension/golang/Go) | `gopls` on `PATH` (`go install golang.org/x/tools/gopls@latest`) |
| Others | syntax highlighting only | add an extension (below) |

To add a language, put its [Open VSX](https://open-vsx.org) extension in `theiaPlugins` in `package.json` (for
example `rust-lang.rust-analyzer`), run `make plugins`, and restart. Language servers only start in trusted
workspaces: if the status bar shows **Restricted Mode**, click it and trust the folder.

## Keyboard shortcuts

`Cmd` on macOS, `Ctrl` elsewhere. Every command is also in the command palette under **Review:**.

| Shortcut | Action |
|---|---|
| `Cmd+Alt+M` | Comment on the line or selection |
| `Cmd+Alt+A` | Ask the agent about the line or selection |
| `Cmd+Alt+↓` / `Cmd+Alt+↑` | Next / previous comment |
| `Cmd+Alt+O` | Go to comment… |
| `Cmd+Enter` / `Esc` | Submit / discard an empty draft |
| `Cmd+Shift+Alt+R` | Show or hide the Review panel |

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `CO_REVIEW_HOME` | `~/.co-review` | Review storage, server registry and log |
| `CO_REVIEW_PORT` | `3000` | Port used when `co-review` starts the browser app |
| `CO_REVIEW_ALLOWED_HOSTS` | *(none)* | Extra host names allowed to open the phone view |

Co-Review ships **Co-Review Dark** and **Co-Review Light** themes and follows the OS setting. Switch themes with
**Preferences: Color Theme**.

## Documentation

- [Design documents](docs/design-docs.md): the design-doc format, anchors, instructing agents
- [Connect your agent](docs/agent-setup.md): skills, the Claude Code plugin, MCP config for each harness
- [Agents](docs/agents.md): ACP vs MCP, how the review loop works, review directories
- [Running and packaging](docs/running-and-packaging.md): build, run, package, install, phone view
- [Architecture](docs/architecture.md): how the Theia extension is organized
- [llms.txt](llms.txt): integration contract for agents

# Contributing

## Build and run from source

```bash
make install                       # npm install + bundled extensions into plugins/
make start REPO=/path/to/repo      # build and run the browser app
make dev                           # rebuild on change; reload the browser
make help                          # all targets
```

The browser app and the desktop app compile native modules for different runtimes. `make browser` and
`make desktop-build` switch between them, so build the one you run.

## Project layout

```
applications/browser     Theia browser app (composition only)
applications/electron    Theia desktop app, electron-builder config, app icon
extensions/review        the review extension
  src/common             review model, protocols, design-document format
  src/node               store, Tree-sitter, ACP client, MCP endpoint, review directories, phone view
  src/electron-node      desktop-only backend bindings
  src/browser            review panel, inline editor UI, document and patch views, themes
bin/                     co-review CLI, install-cli.sh, review server mode
integrations/pi          Pi package: extension + co-reviewer subagent
plugin/                  Claude Code plugin: the agent skills (plugin/skills) + MCP server
.claude-plugin/          plugin marketplace manifest
docs/                    guides and brand assets
llms.txt                 integration contract for agents
```

## Testing

There are no automated tests yet. Changes are verified in the running app and with scripted MCP and ACP clients
against a local server (`make start`, then `co-review mcp` or `http://127.0.0.1:3000/mcp`).

## Packaging

```bash
make package        # dmg/zip (macOS), AppImage/deb (Linux), nsis (Windows) in applications/electron/dist
make install-app    # macOS: package and install locally
```

Signing, notarizing and the packaged layout are covered in [docs/running-and-packaging.md](docs/running-and-packaging.md).

---
