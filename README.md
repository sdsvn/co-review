<div align="center">

<img src="docs/assets/logo.svg" width="88" alt="Co-Review logo" />

# Co-Review

**Review code and designs with Claude Code as your co-reviewer.**

Works just as well with Pi, Codex, Cursor and any MCP agent.

[![Built on Eclipse Theia](https://img.shields.io/badge/built%20on-Eclipse%20Theia-4F46E5?style=flat-square)](https://theia-ide.org)
[![Made for Claude Code](https://img.shields.io/badge/made%20for-Claude%20Code-D97757?style=flat-square)](#made-for-claude-code)
[![Pi package](https://img.shields.io/badge/Pi-package-059669?style=flat-square)](docs/agent-setup.md#pi)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-1E1B4B?style=flat-square)](#install)

[Website](https://sdsvn.github.io/co-review/) · [Install](#install) · [Claude Code](#made-for-claude-code) · [Usage](#usage) · [Agents](#working-with-agents) · [Design docs](docs/design-docs.md) · [Contributing](#contributing)

</div>

Co-Review is a review tool with IDE features, not an IDE. When Claude Code finishes a change, it opens the
change in Co-Review, and you review it the way you would a pull request: comment on lines, functions, designs and
diagrams, and ask about anything you're reading. The agent that did the work answers in the same thread, with links
into the code, and waits for your verdict before it changes anything.

It isn't limited to a diff: review the **entire repository**, a folder, a branch against its base or a single commit,
including code no change touched.

It is built on [Eclipse Theia](https://theia-ide.org) as a platform, so code navigation, search, Git and language
servers are the real thing. Everything unrelated to reviewing is removed.

```
 you ── inline threads, Ask Agent, Submit review ──┐
                                                   ▼
                     Co-Review  (browser or desktop app, Theia)
                      │  reviews in ~/.co-review, outside the repo
          ┌───────────┴────────────┐
   Claude Code plugin        Pi package, or any
   (MCP, live channel,       MCP client (Codex,
    skills, commands,         Cursor, VS Code, …)
    subagent, hook)
          │                        │
   the agent that did the work opens the review and answers your
   comments in a loop — or Co-Review launches an agent for a
   question (ACP: Claude Code, Gemini, OpenCode, Goose, Oh My Pi,
   Pi, Codex — with the model you pick)
```

## Features

- **Inline threads.** Comment on a line, a range, a symbol, a file, a folder or the whole repository. Threads sit
  under the code, and drafts stay open while you read elsewhere.
- **Comments that follow the code.** Each comment is anchored to its symbol and its Tree-sitter tokens. It survives
  edits and moves, and it's marked *outdated* rather than jumping onto code it wasn't about.
- **Agents as reviewers.** Select lines and **Ask Agent**. The answer streams into the thread with the agent's steps,
  and permission requests become buttons for you. Pick the agent's model and reasoning effort from the list it offers.
  Agents answer the way a colleague would, briefly and in plain language.
- **A map of the repository.** A Tree-sitter index of every file and what it defines, built on demand and kept
  outside the repository, takes agents straight to the right code. With a [Graphify](#review-the-whole-repository)
  graph, the repository overview also shows the most connected code and how its parts depend on each other.
- **Design review.** Rendered Markdown, Mermaid diagrams with per-node comments, and a foldable L1 · L2 · L3 design
  tree whose structure is [declared and checked](docs/design-docs.md).
- **Patch review.** `.patch` and `.diff` files open as pull-request pages with PR metadata and line comments.
- **Findings and suggestions.** Agent findings arrive as *proposed*, and you accept or dismiss them. Suggested edits
  are accepted with one click.
- **Whole-repository reviews.** An overview page says where to start. Mark files as viewed (`Cmd+Alt+V`) and see
  how much of each area you've covered, in the panel and in the explorer; a file changed since you viewed it counts
  as unviewed. `/co-review:audit` has the agent do a first pass, with its findings grouped by area.
- **Knowledge bundles.** Review a repository through its OKF bundle: every concept page is a commentable review page,
  and its links open the code at the line.
- **One verdict.** Approve, Request changes or Comment, with a message. The agent gets every open comment in one batch.
- **Phone view.** An installable page to read, reply, accept findings and submit away from your desk.
- **Desktop app and CLI.** `Co-Review.app` with a `co-review` command, or the browser app from a checkout.

## Made for Claude Code

One plugin install makes Claude Code a co-reviewer:

```bash
/plugin marketplace add sdsvn/co-review
/plugin install co-review@co-review
```

- **`/co-review:review`** when a change is ready: Claude opens it in Co-Review, points out what deserves attention,
  answers your comments in their threads, and acts on your verdict.
- **`/co-review:design <task>`** to design first: Claude writes a design doc, you review it, and it implements what
  you approved.
- **`/co-review:audit [focus]`** to review the whole repository: Claude splits it into areas, adds a few proposed
  findings per area for you to accept or dismiss, then answers your questions.
- **Live comments.** With Claude Code's channels on, your questions reach the running session the moment you ask
  them. Claude answers and keeps working ([how](docs/agent-setup.md#live-review-comments-channel)).
- **A background co-reviewer.** The `co-reviewer` subagent keeps answering your review while the main conversation
  fixes things.
- **It remembers.** Opening Claude Code in a repository with open reviews tells it which questions are waiting.
- **Plain requests work too**, through the skills: "review this with me", "design this first".

Co-Review can also launch Claude Code itself for a quick question (**⋯ → Connect agent… → Claude Code**, then
**Ask Agent** on any selection).

### Pi and other agents

**Pi** has a native package: extension, `/co-review`, `/co-review-design`, `/co-review-audit`, a `co-reviewer`
subagent and the skills. It ships inside Co-Review; install it from there:

```bash
# macOS app
pi install /Applications/Co-Review.app/Contents/Resources/app/integrations/pi
# Linux
pi install ~/.local/share/co-review/resources/app/integrations/pi
# a clone of this repository
pi install ./integrations/pi
```

Or click **Connect an Agent** in the Review panel and pick **Pi: package**; Co-Review runs `pi install` for you.

**Codex, Cursor, VS Code, Gemini CLI, Zed, OpenCode, Goose** and any other MCP client: add the MCP server
([config for each](docs/agent-setup.md#other-harnesses-mcp)) and, optionally, the skills with
`npx skills add sdsvn/co-review`.

All of them need the `co-review` command ([Install](#install)). Inside Co-Review, **Connect an Agent** in the Review
panel (or **Review: Add Co-Review to an Agent Harness (MCP)…**) sets any of them up for you.

## Why not pull-request review?

Pull requests review a diff after the fact, and a bot's comments land next to yours with no way to ask it a
follow-up. Co-Review reviews the whole repository, including code the diff doesn't touch, reviews designs before
there is code, and the agent that did the work answers where you asked.

# Using Co-Review

## Install

### macOS and Linux

```bash
curl -fsSL https://sdsvn.github.io/co-review/install.sh | bash
```

Installs the latest release: the desktop app (macOS on Apple silicon, Linux x64 and arm64) and the `co-review` command.
Run it again to update; `CO_REVIEW_VERSION=v0.2.0` pins a release. On Windows, use the installer from the
[latest release](https://github.com/sdsvn/co-review/releases/latest).

```bash
co-review ~/src/my-service     # open a repository
co-review mcp                  # MCP server for agent harnesses; starts the app if needed
```

The command runs on the app's own runtime, so Node.js isn't needed.

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

### Review the whole repository

Start an **entire repository** review. The panel's **Coverage** section shows how many source files you've viewed,
overall and per area; click an area to open its next unviewed file.

- **Overview** (next to the coverage bar, or **⋯ → Repository overview**) opens a page with where to start, the areas
  of the code and how they connect. Its links open the code at the line.
- **Mark a file as viewed** with `Cmd+Alt+V`, the status bar item, or the editor's right-click menu. The explorer
  shows a check on viewed files and the number of open threads on files and folders.
- **Let the agent go first:** `/co-review:audit` (Claude Code) or `/co-review-audit` (Pi) adds proposed findings,
  labelled by area. **By area** in the panel groups them; accept or dismiss each.

For a richer overview, build a [Graphify](https://pypi.org/project/graphifyy/) graph first (`graphify update .`, code
only, no LLM). Co-Review reads `graphify-out/graph.json`: the most connected code, its clusters, and a diagram of the
links between them. Without it, the overview uses the Tree-sitter repo map.

To review through an **OKF** knowledge bundle, have the agent open it: `open_review({ dir: "<repo>/okf" })`. Each
concept page opens as a review page with its type and the code it describes, and agents see which page you commented on.

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

| | The agent brings Co-Review | Co-Review launches the agent (ACP) |
|---|---|---|
| Best for | Claude Code or Pi did the work and hands it to you | Asking questions while you review |
| Setup | The Claude Code plugin, the Pi package, or MCP ([above](#made-for-claude-code)) | **⋯ → Connect agent…**: Claude Code, Gemini, OpenCode, Goose, Oh My Pi, Pi or Codex, then the model |
| Start | `/co-review:review` or `/co-review:audit` (Claude Code), `/co-review` or `/co-review-audit` (Pi) | Select code → **Ask Agent** (`Cmd+Alt+A`) |

When the agent brings Co-Review, it knows what it just built and why. It opens the review, adds findings for real
risks, then answers your comments in a loop until you submit. The panel shows it as **listening**, or **busy** while
it works; your questions are delivered when it checks in.

The full contract is in [llms.txt](llms.txt). Skills for other agents: `npx skills add sdsvn/co-review`, or
**Review: Install Agent Skills…** in the app.

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
| `Cmd+Alt+V` | Mark the file as viewed (or not) |
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

The docs are published at **https://sdsvn.github.io/co-review/** (built from `docs/` by `site/`).

- [Design documents](docs/design-docs.md): the design-doc format, anchors, instructing agents
- [Connect your agent](docs/agent-setup.md): the Claude Code plugin, the Pi package, skills, MCP config for other harnesses
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
  src/node               store, Tree-sitter, repo map and overview, ACP client, MCP endpoint, review directories, phone view
  src/electron-node      desktop-only backend bindings
  src/browser            review panel, inline editor UI, document and patch views, themes
bin/                     co-review CLI, install-cli.sh, review server mode
integrations/pi          Pi package: extension, /co-review-design and /co-review-audit prompts, co-reviewer subagent
plugin/                  Claude Code plugin: MCP server + channel, skills, commands (review, design, audit), co-reviewer subagent, SessionStart hook
.claude-plugin/          plugin marketplace manifest
docs/                    guides and brand assets (the source of the website's guides)
site/                    website and docs: Astro + Starlight, deployed to GitHub Pages
llms.txt                 integration contract for agents
```

## Testing

There are no automated tests yet. Changes are verified in the running app and with scripted MCP and ACP clients
against a local server (`make start`, then `co-review mcp` or `http://127.0.0.1:3000/mcp`).

## Packaging

```bash
make package        # dmg/zip (macOS), tar.gz/AppImage/deb (Linux), nsis (Windows) in applications/electron/dist
make install-app    # macOS: package and install locally
```

Releases are built by `.github/workflows/release.yml` on macOS (arm64), Linux (x64, arm64) and Windows (x64). Pushing
a `v*` tag publishes them as a GitHub release, which `install.sh` downloads.

Signing, notarizing and the packaged layout are covered in [docs/running-and-packaging.md](docs/running-and-packaging.md).


## Website

`site/` is the website and docs (Astro + Starlight). Its guides are generated from `docs/*.md` and `llms.txt` at build
time, and `starlight-llms-txt` publishes `llms.txt`, `llms-full.txt` and `llms-small.txt` for agents.

```bash
cd site && npm install
npm run dev        # http://localhost:4321/co-review/
npm run build      # sync docs, type-check, build into site/dist
```

Pushes to `main` that touch `site/`, `docs/` or `llms.txt` deploy it through `.github/workflows/site.yml`.

---
