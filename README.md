<div align="center">

<img src="docs/assets/logo.svg" width="88" alt="Co-Review logo" />

# Co-Review

**You built it with your coding agent. Review the whole repository together, and make it yours again.**

Made for Claude Code. Native packages for Pi and Oh My Pi. Works with any MCP agent.

[![Built on Eclipse Theia](https://img.shields.io/badge/built%20on-Eclipse%20Theia-4F46E5?style=flat-square)](https://theia-ide.org)
[![Made for Claude Code](https://img.shields.io/badge/made%20for-Claude%20Code-D97757?style=flat-square)](#claude-code)
[![Pi and Oh My Pi](https://img.shields.io/badge/Pi%20%C2%B7%20Oh%20My%20Pi-packages-059669?style=flat-square)](#pi-and-oh-my-pi)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-1E1B4B?style=flat-square)](#install)

[Website](https://sdsvn.github.io/co-review/) · [Install](#install) · [Connect your agent](#connect-your-agent) · [Usage](#usage) · [Docs](#documentation) · [Contributing](#contributing)

</div>

Building with an agent is fast. Change by change, each diff looks fine, and months later the codebase works but no
longer feels like yours: you don't know where things live or why they're built the way they are. Co-Review is for
getting it back. Once in a while, open the **entire repository** as a review and read it with the agent that built it,
the way you'd read a pull request. Comment on a line, a function, a file or a folder, and ask why; the agent answers in
the thread, from the code. An overview shows where to start, the agent can take a first pass with findings grouped by
area, and coverage tracks what you've read.

But whole-repository review is only one of three jobs Co-Review does:

- **A whole repository** — read the entire codebase with the agent (as above), to get it back in your head or to check your own work.
- **A change or pull request** — a single change, a branch against its base, a commit, or a `.patch` / `.diff` file opened as a pull-request page, with line comments, suggested edits and one verdict.
- **A design, before the code** — review the plan, not the source: a design written as an L1 · L2 · L3 tree with Mermaid diagrams, revised from your comments before anything is built.

Co-Review is a review tool with IDE features, not an IDE. It's built on [Eclipse Theia](https://theia-ide.org),
so navigation, search, Git and language servers are the real thing; everything unrelated to reviewing is removed.

```
 you ── inline threads, Ask Agent, Submit review ──┐
                                                   ▼
                     Co-Review  (desktop app or browser)
                      │  reviews live in ~/.co-review, outside the repo
       ┌──────────────┼─────────────────────┐
  Claude Code     Pi / Oh My Pi        any MCP client
  plugin          packages             (Codex, Cursor, VS Code, …)
       │              │                     │
  the agent that did the work opens the review and answers your comments —
  or Co-Review launches an agent for a question (ACP: Claude Code, Codex,
  Gemini, OpenCode, Goose, Pi, Oh My Pi, with the model you pick)
```

## Features

- **Whole-repository reviews** with an overview page, per-area coverage (`Cmd+Alt+V` marks a file viewed) and an
  agent first pass whose findings arrive *proposed* for you to accept or dismiss.
- **Inline threads** on a line, range, symbol, file, folder or the whole repository. Comments are anchored to their
  symbol and Tree-sitter tokens, so they follow the code through edits and are marked *outdated* instead of drifting.
- **Agents as reviewers.** Select code and **Ask Agent**: the answer streams into the thread with the agent's steps,
  and permission requests become buttons.
- **A repository map** (Tree-sitter, built on demand, stored outside the repo) that takes agents straight to the
  right code, enriched by a [Graphify](https://pypi.org/project/graphifyy/) graph when there is one.
- **Design review**: rendered Markdown, Mermaid diagrams with per-node comments, and an L1 · L2 · L3 design tree
  whose structure is [declared and checked](docs/design-docs.md).
- **Patches and knowledge bundles**: `.patch` / `.diff` files open as pull-request pages; OKF bundles open as
  commentable pages linked to the code.
- **One verdict** — Approve, Request changes or Comment — and the agent gets every open comment in one batch.
- **Phone view** to read, reply and submit away from your desk.

## Install

**macOS and Linux:**

```bash
curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
```

This installs the desktop app (macOS on Apple silicon, Linux x64 and arm64) and the `co-review` command, which runs on
the app's own runtime (no Node.js needed). Run it again to update; `CO_REVIEW_VERSION=v0.2.0` pins a release.
**Windows:** use the installer from the [latest release](https://github.com/sdsvn/co-review/releases/latest).

```bash
co-review ~/src/my-service     # open a repository
```

To build from source instead, see [Contributing](#contributing).

## Connect your agent

Every integration launches Co-Review through the `co-review` command, so install it first. Inside Co-Review,
**Connect an Agent** in the Review panel sets up any of the harnesses below for you.

### Claude Code

```bash
/plugin marketplace add sdsvn/co-review
/plugin install co-review@co-review
```

| Command | What Claude does |
|---|---|
| `/co-review:audit [focus]` | Reviews the whole repository with you: a few proposed findings per area, then answers your questions |
| `/co-review:review` | Opens a change in Co-Review, points out what deserves attention, answers your comments and acts on your verdict |
| `/co-review:design <task>` | Writes a design doc, has you review it, implements what you approved |

With Claude Code's [channels](docs/agent-setup.md#live-review-comments-channel) on, your questions reach the running
session the moment you ask. A background `co-reviewer` subagent can keep answering while the main conversation fixes
things, and a session hook tells Claude which reviews are waiting. Plain requests ("review this with me") work too.

### Pi and Oh My Pi

Both ship inside Co-Review, so there is nothing to clone:

```bash
co-review setup pi     # or: co-review setup omp
```

You get the same `co_review_*` tools and `/co-review`, `/co-review-design` and `/co-review-audit` commands in both,
plus a `co-reviewer` agent and the skills. In Oh My Pi, only the main session listens for your questions, not every
task subagent. Details: [Pi and Oh My Pi](docs/agent-setup.md#pi-and-oh-my-pi).

### Other agents

Codex, Cursor, VS Code, Gemini CLI, Zed, OpenCode, Goose and any other MCP client: add the MCP server
(`co-review mcp`, [config for each](docs/agent-setup.md#other-harnesses-mcp)) and, optionally, the skills with
`npx skills add sdsvn/co-review`. The full contract is in [llms.txt](llms.txt).

### Or let Co-Review launch the agent

For quick questions while you review, **⋯ → Connect agent…** in the Review panel starts an ACP agent (Claude Code,
Codex, Gemini, OpenCode, Goose, Pi or Oh My Pi) with the model you pick. Then select code → **Ask Agent**
(`Cmd+Alt+A`). See [Agents](docs/agents.md) for when to use which.

> [!IMPORTANT]
> Agents and language servers use Co-Review's environment. The desktop app loads your login shell's `PATH`, and
> `co-review` passes on the terminal's.

## Usage

Open the **Review** panel (status bar, or `Cmd+Shift+Alt+R`) and pick what to review: the entire repository, the
current file, a branch against its base, or a commit. Or right-click files or folders → **Start Review of Selection**.

| To comment on | Do this |
|---|---|
| A line or range | Click **+** in the gutter (drag for a range), or select and press `Cmd+Alt+M` |
| A symbol | Right-click inside it → **Add Review Comment on Symbol** |
| A file or folder | Right-click it in the explorer → **Add Review Comment** |
| The repository | **⋯** in the Review panel → **Comment on the repository** |

Once an agent is connected, **Ask Agent** appears next to commenting. Reply, resolve and collapse threads inline; the
panel lists them by file (**Open / Proposed / Resolved / All**), and **⋯** has the rest.

- **Whole repository:** the panel's **Coverage** shows what you've viewed, overall and per area. **Overview** says
  where to start. `/co-review:audit` (or `/co-review-audit` in Pi and Oh My Pi) lets the agent go first.
- **Designs:** `index.markdown` and `*.pseudocode.md` open rendered (any `.md`: **Open With → Review (rendered)**).
  Comment on text, diagram nodes and edges, or design steps. See [Design documents](docs/design-docs.md).
- **Phone:** **⋯ → Open mobile view**, or `http://<host>:<port>/m/` (local connections only;
  [over Tailscale](docs/running-and-packaging.md#phone-view)).

### Keyboard shortcuts

`Cmd` on macOS, `Ctrl` elsewhere. Every command is also in the command palette under **Review:**.

| Shortcut | Action |
|---|---|
| `Cmd+Alt+M` / `Cmd+Alt+A` | Comment on / ask the agent about the line or selection |
| `Cmd+Alt+↓` / `Cmd+Alt+↑` | Next / previous comment |
| `Cmd+Alt+O` | Go to comment… |
| `Cmd+Alt+V` | Mark the file as viewed (or not) |
| `Cmd+Enter` / `Esc` | Submit / discard an empty draft |
| `Cmd+Shift+Alt+R` | Show or hide the Review panel |

Language support (TypeScript, JavaScript, JSON, Markdown, PHP, Go with `gopls`, and how to add more), configuration
variables and themes are covered in [Running and packaging](docs/running-and-packaging.md).

## Documentation

Published at **https://sdsvn.github.io/co-review/**.

- [Connect your agent](docs/agent-setup.md): Claude Code plugin, Pi and Oh My Pi packages, skills, MCP config
- [Agents](docs/agents.md): ACP vs MCP, the review loop, review directories
- [Design documents](docs/design-docs.md): the design-doc format and how to instruct agents
- [Running and packaging](docs/running-and-packaging.md): build, run, package, languages, configuration, phone view
- [Architecture](docs/architecture.md): how the Theia extension is organized
- [llms.txt](llms.txt): the integration contract for agents

## Contributing

Requires Node.js 20+, Git and a C/C++ toolchain (Xcode command-line tools on macOS, `build-essential` on Linux).

```bash
make install                       # npm install + bundled extensions into plugins/
make start REPO=/path/to/repo      # browser app on http://127.0.0.1:3000
make desktop REPO=/path/to/repo    # desktop app
make dev                           # rebuild on change
make install-cli                   # `co-review` command running from this checkout
make package                       # installers in applications/electron/dist
make help                          # all targets
```

The browser and desktop apps compile native modules for different runtimes; `make browser` and `make desktop-build`
switch between them, so build the one you run. Agent instructions (skills, commands, subagents, tool text) are
written once in [`prompts/`](prompts/README.md): edit them there and run `make prompts`. There are no automated tests yet: changes are verified in the running
app and with scripted MCP and ACP clients against a local server. Releases are built by
`.github/workflows/release.yml` when a `v*` tag is pushed.

```
applications/            Theia browser and desktop apps (composition, electron-builder config)
extensions/review        the review extension (common · node · electron-node · browser)
bin/                     co-review CLI, install-cli.sh, review server mode, gen-prompts.mjs
prompts/                 the agent prompts, written once; `make prompts` copies them everywhere
plugin/, .claude-plugin/ Claude Code plugin and its marketplace manifest
integrations/shared      the Pi / Oh My Pi extension and slash commands
integrations/pi, omp     Pi and Oh My Pi packages (harness adapter, co-reviewer agent)
docs/                    guides (the source of the website's docs)
site/                    website: Astro + Starlight (`cd site && npm run dev`), deployed from main
llms.txt                 integration contract for agents
```

See [Architecture](docs/architecture.md) and [Running and packaging](docs/running-and-packaging.md) for details.
