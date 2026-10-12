# Architecture

Co-Review is a fork of [Lapce](https://lapce.dev), a native editor written in Rust. Lapce provides the IDE: the
editor, the file explorer, search, Git, the terminal, and language servers through its plugins. Co-Review adds the
review in its own crates (`crates/review_*`) and in one module of the app (`lapce-app/src/review/`). Lapce's own files
carry only small hooks, each marked `// co-review:` and listed in [the fork surface](co-review/fork-surface.md).

## Directory layout

```
lapce-app/             Lapce's app (Floem UI), with Co-Review's hooks
  src/review.rs        starts the review core with the app; connects it to windows
  src/review/          the review UI: panel, inline threads, review pages, pickers
lapce-core/            Lapce's core (rope, syntax, directories)
lapce-proxy/           Lapce's proxy (files, Git, language servers, plugins)
lapce-rpc/             messages between the app and the proxy
crates/review_*        the review core (no UI), one crate per concern
packaging/             release assets: package.sh, the macOS Info.plist, the Inno Setup script, icons
bin/install-cli.sh     links the `co-review` command into PATH
bin/gen-prompts.mjs    copies prompts/ into skills, commands, agents, the MCP server's text and docs
prompts/               agent prompts, written once
integrations/shared    Pi / Oh My Pi extension and slash commands, shared by both packages
integrations/pi        Pi package: adapter, subagent
integrations/omp       Oh My Pi package: adapter, task agent
plugin/                Claude Code plugin: MCP server + channel, skills, commands, subagent, SessionStart hook
                       (.claude-plugin/ is its marketplace)
```

## One process

The app runs the review core in its own process. When it starts, it loads the store and starts the MCP server, then
opens windows. Started with `--background` (as `co-review mcp` does for an agent), it opens no window until a review
is shown or the app is activated.

The review crates know nothing about windows. They use plain threads and channels, and their tests run headless.
`lapce-app/src/review/` turns their events into Floem signals on the UI thread and runs file, Git and store work off
it.

## The review core (`crates/`)

- **`review_model`**: reviews, threads, locations, anchors and the payloads agents see. The JSON field names are the
  ones the Theia app wrote, and unknown fields are kept, so either app reads the other's data.
- **`review_store`**: reviews as JSON files under `~/.co-review/workspaces/<hash>/reviews/` (`CO_REVIEW_HOME`
  overrides the folder). Changes are applied one at a time per review, saved, and broadcast to every window and
  agent.
- **`review_anchor`**: comments that follow the code (below).
- **`review_index`**: the repository map (a Tree-sitter outline of every source file), its areas, and coverage.
  Symbols are cached by content (the git blob hash) in a cache shared by every worktree of the repository.
- **`review_graph`**: runs [Graphify](https://graphify.net) (`graphify update`, code only, no LLM) when it is
  installed, shares its output across the repository's worktrees, and builds the repository overview and a change's
  blast radius from the graph.
- **`review_bundle`**: review directories: the document, patches, `PR.md`, sidecar findings, suggested edits, OpenSpec
  changes and the `review.json` state file.
- **`review_design`**: the design-document format check ([Design documents](design-docs.md)).
- **`review_markdown`**: rendered Markdown pages as blocks to draw, and the page's text that text anchors use.
- **`review_mermaid`**: Mermaid diagrams parsed and laid out (ported from [Ferrite](https://github.com/OlaProeis/Ferrite)),
  with the node and edge ids comments anchor to.
- **`review_pages`**: every review's overview page, written at once with what is cheap; the repository overview and
  the blast radius fill in when they are ready.
- **`review_code`**: a change review's code: the repository itself for your own change, or a git worktree of a pull
  request's head under `~/.co-review/worktrees/`, its changed neighbourhood first. It also reads base versions.
- **`review_github`**: posts a pull request's review through the GitHub CLI (`gh`).
- **`review_mcp`**: the MCP server for agents in their own harness: Streamable HTTP at
  `http://127.0.0.1:<port>/mcp?root=<repo>`, local connections only, recorded in `~/.co-review/server.json`. The tools
  are in [`llms.txt`](../llms.txt). It tracks which agents are listening, the reviewer's pending decisions, and the
  Claude Code channel.
- **`review_agents`**: the ACP client. It launches the agent the reviewer connects, with one session per thread, and
  streams answers, tool activity and permission requests into the threads.
- **`review_cli`**: the `co-review` command. It opens a repository, runs the stdio MCP bridge (`co-review mcp`), and
  provides `status`, `setup pi|omp` and `logs`. It starts fast: the `mcp` path loads no UI code.

## The review UI (`lapce-app/src/review/`)

- **The Review panel** (`panel.rs`, `submit.rs`, `picker.rs`, `commands.rs`) shows the review switcher, the scope, the
  agent, drafts, threads by file or by area, the filters, coverage, and Submit review. The pickers (Go to Comment…, the
  ⋯ menu, Connect agent…) use Lapce's palette. The `Review:` commands live in `commands.rs`.
- **Inline threads** (`inline.rs`, `thread.rs`, `drafts.rs`): Floem's editor can't place widgets between lines.
  So a line with threads gets blank rows of phantom text, and an overlay draws the threads there. The gutter has
  thread markers and **+**. Drafts are saved per review and survive a restart.
- **Review pages** (`page.rs`) are editor tabs, like Settings and Keymap:
  - the overview and document pages (`document.rs`, `markdown_view.rs`, `design_tree.rs`, `mermaid.rs`);
  - the pull-request page (`pull_request.rs`), a scrolling list of per-file diffs with thread rows between lines.
- **The change in the code** (`changed_files.rs`, `code.rs`): the project panel colours the changed files and can
  show only them. A changed file opens as base and head side by side. From the pull-request page, a line number opens
  the file, and go to definition and hover use the language server.
- **Windows** (`windows.rs`, `stale.rs`): there is one window per repository, and showing a review opens or focuses
  it. Closing a review's window, or quitting, tells its agents (`await_reviewer` returns `closed`). In an app started
  in the background, a window an agent showed closes after a countdown once that agent is done. An app that an update
  replaced on disk quits when it can.

## Locations that follow the code

A comment tracks what happened to the code it is on, and says so when that code is gone. Line, range and document
text comments carry a **tracked range**: character offsets in a version of the file (a content hash), the text and 40
characters of context on each side, and a state, separate from the thread's status:

- `active`: the text is there unchanged (it may have moved);
- `modified`: it changed but is still identified (the original quote is kept);
- `removed`: deleted, or not located reliably;
- `ambiguous`: several places fit about equally well.

Removed and ambiguous comments are **out of scope**: never placed on code, shown in the panel's Out of scope filter,
and back in scope if the code changes again and they can be placed.

`review_anchor` resolves a review's anchors whenever the review is read (the window, `get_review`, `await_reviewer`)
or a commented file changes. It starts from the snapshot of the anchor's version, content-addressed under
`~/.co-review/workspaces/<hash>/snapshots/`. It computes the edits to the current file with diff-match-patch (a line
diff, then characters within changed lines) and maps the range through them.

- Same text at the mapped range: active.
- Mostly kept and similar: modified, unless the unchanged text sits, with its context, where the edits wrote (a moved
  block): then it is active there.
- Otherwise it searches. Exact occurrences are scored by matching context: one clear best is active, several close
  ones are ambiguous. Then it tries a fuzzy match near the mapped position.
- The search only looks in text the edits wrote, never on code that was already there untouched. If nothing fits,
  the comment is removed.

An edit Co-Review applies itself (an accepted suggestion) is mapped exactly, without a diff.

In an open editor, comments follow every edit before it is saved. A comment is placed by its symbol, by its exact text,
by token sequence (which survives reformatting), or by text for languages without a grammar. Otherwise it is shown as
outdated where it was, with its original code.

Tree-sitter is used for what makes locations durable, and for the repository map. Navigation (definition,
references) stays with the language servers.

## Tests

Each crate's tests replay fixtures recorded from the Theia app that Co-Review was built on before (`tests/fixtures`,
and the MCP sessions in `crates/review_mcp/tests/conformance/`), so agents and existing data see the same behaviour.
`make test` runs them; CI (`.github/workflows/rust.yml`) runs them on macOS, Linux and Windows.
