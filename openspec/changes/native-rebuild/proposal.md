## Why

Co-Review runs on Eclipse Theia (Electron, Monaco in the DOM). It is slow where a review is felt most: the window
takes seconds to appear, large diffs and files full of threads scroll poorly, and indexing runs single-threaded in
Node on Tree-sitter compiled to WASM. Rebuilding Co-Review on a fork of a native Rust editor keeps the product and
replaces the slow foundation. Which editor is a design decision (`design.md`); these specs hold for any of them.

## What Changes

- Co-Review becomes a fork of a native editor. Everything Co-Review does today is rebuilt on top of it, as specified
  by the capabilities below. Plain IDE features (editing, navigation, search, Git, language servers, terminal) come
  from the host and are not specified; nothing new is added.
- **BREAKING:** the browser app, the phone view (`/m/`) and the review server (`co-review-server`) are dropped
  for good. Co-Review is a desktop app. `open_review` shows the review in the app's window and returns
  a `note`, never a `url`.
- One behaviour is corrected while porting: the repository map and the Graphify graph are built once per
  repository and shared by its worktrees and across restarts, instead of rebuilt per folder.
- The Theia app is retired when the new app ships.
- Unchanged: the MCP tool contract (`llms.txt`), the review data under `~/.co-review`, the `co-review` command, and
  the integrations (Claude Code plugin, Pi and Oh My Pi packages, skills and prompts).

## Capabilities

### New Capabilities

- `review-data`: reviews, threads, messages and their storage under `~/.co-review`; review kinds and lifecycle.
- `comment-anchoring`: comment locations, and how comments follow the code through edits (active, modified, removed, ambiguous).
- `inline-review`: threads in the editor, starting a thread, drafts, moving between threads, links in messages.
- `review-panel`: the Review panel, its filters and grouping, Submit review, and viewed files and coverage.
- `repository-review`: the repository map, areas, Graphify graph, repository overview and findings by area.
- `change-review`: the pull-request page for a diff, stepping from the diff into the code, code folders, the changed files in the project panel, blast radius and suggested edits.
- `github-posting`: posting a review to a GitHub pull request with a preview.
- `document-review`: rendered Markdown, Mermaid and design-tree comments, the design format check, suggested document edits, review directories, OKF bundles and OpenSpec cards.
- `overview-page`: every review's front page.
- `mcp-agents`: the MCP server and its tools for agents running in their own harness, delivery, presence and the Claude Code channel.
- `acp-agents`: agents Co-Review launches over ACP to answer in threads.
- `desktop-app`: background start, one window per repository, window lifecycle, themes, the review-focused shell and branding.
- `cli`: the `co-review` command, the stdio MCP bridge, setup, status, logs and stale-version handling.

### Modified Capabilities

None: no capabilities are specified in `openspec/specs/` yet. These specs become the baseline when this change
is archived.

## Impact

- New repository content: the editor fork with `review_*` crates (see `design.md`), with `integrations/`, `plugin/`,
  `prompts/`, `site/`, `docs/` and `llms.txt` carried over.
- Prompts, skills and `llms.txt`: wording that asks the agent to give the reviewer a URL changes to say the review
  is open in Co-Review.
- Removed: the Theia browser and Electron apps, `make start`, `/#<repo>` links.
- License: follows the host editor's license; see `design.md`.
- Install: `install.sh` and the release workflow switch to the new app's builds.
