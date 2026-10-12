# Building Co-Review on Zed: instructions

For whoever builds the Zed fork, person or agent. The OpenSpec change [`openspec/changes/native-rebuild`](openspec/changes/native-rebuild/proposal.md) says what to build
(`specs/`), why (`design.md`) and in which order (`tasks.md`); this file says how. When they disagree, the specs win
and this file is fixed.

## 1. The fork

### Repository setup

```bash
git clone https://github.com/zed-industries/zed co-review-zed
cd co-review-zed
git remote rename origin upstream
git remote add origin <co-review-zed repository URL>
git checkout -b main <latest Zed stable tag>
```

- **Upstream base: Zed stable tags** (`v0.xxx.y`), not upstream `main`. Every week, merge the newest stable
  tag into `main` (merge, never rebase: the history must show what came from upstream).
- Co-Review's own files carried over from the Theia repository live at the top level, beside Zed's:
  `integrations/`, `plugin/`, `.claude-plugin/`, `prompts/`, `site/`, `docs/co-review/`, `llms.txt`, `install.sh`.
- Replace Zed's `AGENTS.md`, `CLAUDE.md` and `GEMINI.md` with the fork's own (this file's rules, shortened), and
  keep Zed's Rust and GPUI guidance from them in a section of their own. Zed's agent files are written for Zed
  contributors and include rules that don't apply to the fork.

### The fork's edit budget

Merging upstream every week stays cheap only if the fork rarely edits Zed's own files.

- **New code goes in new crates** named `review_*` (see the crate table in `openspec/changes/native-rebuild/design.md`). A new crate never needs a
  merge.
- **An edit to a Zed crate is an extension point, not a feature.** Add the smallest hook that lets a `review_*`
  crate do the work: a trait, a callback, a registration function, a `pub` on a private item. Never write review
  logic inside a Zed crate.
- **Mark every edit** to a Zed file with a `// co-review:` comment on the line or block that says why the hook is
  needed, so `git grep "co-review:"` lists the whole fork surface.
- **Keep the list in `docs/co-review/fork-surface.md`**: file, hook, why, and whether an upstream PR could make it
  unnecessary. Review the list at every upstream merge.
- **Expected hooks** (anything beyond these needs a decision in `design.md` first):

| Zed crate | Hook | For |
|---|---|---|
| `zed` | register the review crates; `--background`; app name, icons, bundle id, update channel; default settings and keymap | desktop-app |
| `editor` | let an external crate add a gutter affordance (the **+**) and a selection toolbar entry, as breakpoints and code actions do | inline-review |
| `project_panel` | an entry decoration provider (colour, suffix text such as `+12 −3`, viewed mark) and an entry filter | change-review, review-panel |
| `workspace` | window close and app quit notifications with the workspace's root; single-instance "open or focus" by root | desktop-app |
| `mermaid_render` | node and edge ids with their bounding boxes (if the SVG output doesn't already carry them) | document-review |
| `auto_update`, `client`, `telemetry` | point at Co-Review's release feed; disable Zed sign-in and telemetry | desktop-app |

### Branding and shell

- Product name **Co-Review**, binary `co-review`, bundle id `dev.co-review.app`, data in `~/.co-review` (Zed's own
  config folder renamed to `~/.config/co-review` so it never shares settings with an installed Zed).
- Remove Zed's logos, names and URLs from everything user-visible; keep the copyright notices Zed's licenses
  require.
- Default `settings.json` turns off the debugger, tasks, REPL, collab, edit prediction and the AI assistant panel;
  the `review_mode` feature flag hides their actions from the command palette and menus. Don't delete their crates.

## 2. Compatibility contract

Agents and existing users must not notice the switch. These are fixed and covered by tests; change them only
together with every integration and with a version bump.

1. **MCP tools**: names, arguments, defaults, result shapes, error texts and the server instructions, exactly as in
   `llms.txt` and `extensions/review/src/node/co-reviewer-mcp.ts` of the Theia app (the reference).
2. **Data**: `~/.co-review` layout (`workspaces/<sha256(root uri)[0:16]>/reviews/*.json`, `workspace.json`,
   `snapshots/`, `pages/`, `worktrees/`, `server.json`, `logs/`), `CO_REVIEW_HOME`, and every JSON shape in
   `review-model.ts`. The new app reads data the Theia app wrote, and the other way round, so users can switch back.
3. **Review directories**: `review.json`, `PR.md`, `<patch>.comments.json(l)` and the anchor formats in `llms.txt`.
4. **The `co-review` command**: subcommands, flags, exit codes and the stdio MCP bridge's behaviour (handshake
   answered locally, app started on the first tool call, reconnect, exit with the harness).
5. **One deliberate change**: there is no browser app (`design.md`, decision 11). `open_review` returns a `note` and never a
   `url`; update the prompts, skills and `llms.txt` that tell the agent to "give the reviewer the URL" so they say
   to tell the reviewer the review is open in Co-Review. This is the only contract change.
6. **Prompts**: `prompts/` stays the single source; `bin/gen-prompts.mjs` (or its Rust port) copies them into
   skills, commands, agents, app text and docs. Never edit a generated copy.

**How it's checked.** Before writing a Rust crate, record the reference behaviour from the Theia app:

- **MCP sessions**: a recorder in the Theia app's MCP endpoint writes every request and response of the four
  workflows (audit, change, PR, design) to `tests/conformance/mcp/*.jsonl`. The Rust server must replay them with
  equal results (ids, timestamps and URLs normalized).
- **Corpora**: anchors (`anchor-corpus.mjs` cases), patches (every `.patch` the Theia tests use plus real ones),
  design documents (`docs/design-docs.md` examples plus every design from past reviews), review JSON (a sanitized
  copy of real `~/.co-review` data). Each has an expected-output file produced by the TypeScript code.
- Conformance tests run in CI on every commit.

## 3. Porting the TypeScript

Port behaviour, not structure: read the TypeScript, its tests and its docs, then write idiomatic Rust. Where the
TypeScript works around Theia (DI, RPC between frontend and backend, Monaco view zones), drop the workaround.

| Theia app (extensions/review/src) | Zed fork crate | Notes |
|---|---|---|
| `common/review-model.ts`, `review-protocol.ts` | `review_model` | serde with the same field names (`#[serde(rename_all = "camelCase")]`); no RPC layer needed |
| `common/anchor-tracking.ts`, `node/anchor-tracker.ts`, `anchor-snapshots.ts` | `review_anchor` | diff-match-patch → `similar` or `dissimilar`; CodeMirror change sets → own range mapping over the diff; same thresholds |
| `node/syntax-service-impl.ts`, `browser/syntax-symbols.ts` | `review_syntax` | Zed's grammars and outline queries replace `@vscode/tree-sitter-wasm` (native, much faster) |
| `node/review-store.ts`, `participants.ts` | `review_store` | one async mutex per review; events through a GPUI entity or a broadcast channel |
| `node/repo-index.ts`, `code-folders.ts` (areas) | `review_index` | cache symbols by **git blob hash** (`git ls-files -s`), shared per repository (`git rev-parse --git-common-dir`); never by mtime |
| `node/graphify.ts`, `repo-overview.ts` | `review_graph` | one Graphify output per repository; seed a new worktree's `graphify-out/` from it (copy-on-write) before `graphify update`; remember the last build on disk |
| `node/overview-page.ts` | `review_pages` | write cheap parts first, slow sections later (as the Theia app does since 0.6.0) |
| `node/bundle-service.ts`, `review-payloads.ts`, `common/patch.ts` | `review_bundle` | file watching through Zed's `fs` watcher |
| `common/design-format.ts` | `review_design` | |
| `node/github.ts` | `review_github` | still shells out to `gh` |
| `node/code-folders.ts` | `review_code` | git worktrees, sparse checkout, base file reads |
| `node/co-reviewer-mcp.ts`, `agent-coordination.ts` | `review_mcp` | `rmcp` with its Streamable HTTP server; channel push for Claude Code |
| `node/acp-agent-service.ts`, `agent-setup.ts` (agent list) | `review_agents` | built on `agent_servers` and `acp_thread`; no new ACP client |
| `node/mobile-review.ts`, `mobile-page.ts`, `bin/review-server.mjs` | `review_web` | serve the same HTML; same REST API |
| `bin/co-review.mjs`, `node/agent-setup.ts` (setup), `debug-log.ts` | `review_cli` | the command must start fast: no GPUI in the `mcp` bridge path |
| `browser/*` (panel, editor decorator, inline zone, widgets, document and patch widgets, theme) | `review_ui` | GPUI; one module per surface, see section 4 |
| `electron-main/*` | `zed` hooks + `review_ui` | `--background`, single instance, window lifecycle |

## 4. Rust and GPUI conventions

Follow Zed's own conventions, so the fork reads like Zed and upstream code is a reliable example:

- No `unwrap()` or panicking indexing outside tests; propagate errors with `?`; never discard a `Result` silently
  (use `.log_err()` when ignoring is right); errors that matter to the reviewer reach the UI as a message.
- Library roots named after the crate (`[lib] path = "src/review_store.rs"`), no `mod.rs` files.
- Full-word names; comments only for a non-obvious *why*.
- GPUI: state in entities, foreground work with `cx.spawn`, heavy work with `cx.background_spawn`; keep every
  `Task` (store it, await it or `detach_and_log_err`); never update an entity while it's being updated.
- UI from Zed's `ui` crate components (buttons, lists, modals, tooltips, icons) so the review looks native and
  follows the theme.
- Settings through Zed's settings system (`review` section in `settings.json`), keys under `Review:` in the
  command palette, keybindings in the default keymap (same keys as today: `Cmd+Alt+M`, `Cmd+Alt+A`,
  `Cmd+Alt+↓/↑`, `Cmd+Alt+O`, `Cmd+Alt+V`; `Ctrl` instead of `Cmd` off macOS).

**Learn from the closest Zed code before writing a surface:**

| Building | Read first |
|---|---|
| Inline threads in the editor | `editor` block decorations; `agent_ui`'s inline assistant; `diagnostics` inline blocks |
| Patch page, changed files | `git_ui::project_diff`, `git_ui::multi_diff_view`, `multi_buffer`, `buffer_diff` |
| Review panel | `git_ui::git_panel`, `outline_panel` |
| Pickers (Go to Comment, review switcher, agent picker) | `picker`, `file_finder` |
| Rendered documents | `markdown`, `markdown_preview`, `mermaid_render` |
| Streaming agent answers, permission buttons | `agent_ui` with `acp_thread` |
| Project panel decorations | `project_panel` git status colouring |

## 5. Performance rules

Performance is the reason for the fork; these rules hold everywhere.

1. **Never block the foreground thread.** File reads, git, Tree-sitter parsing, diffing, JSON of a whole review,
   Graphify: always on the background executor. Foreground work in review code stays under a few milliseconds;
   anything over 100 ms is logged with what it was doing.
2. **Never make the reviewer wait on an index.** Show what's cheap now; fill in slow sections when ready (the
   overview page, the blast radius, the repo map in an ACP prompt after 2 s).
3. **Build once, reuse everywhere.** Caches are per repository and keyed by content (git blob hash), shared by every
   worktree, and persisted, so a restart or a new worktree costs only what changed.
4. **Render only what's on screen.** Thread blocks, patch pages and the panel use Zed's virtualized lists and
   multibuffers; a closed or folded thread renders as a one-line block.
5. **Incremental updates.** A store change updates the threads it touched, not the whole panel or page; anchor
   tracking reruns only for the files that changed.
6. **Measure.** Every performance target in `design.md` has a benchmark in `crates/review_benchmarks`, run in CI on the benchmark
   repositories; a regression of more than 20 % fails the build. Use Zed's `ztracing` spans around every
   review operation that touches the disk, git or a parser.

## 6. Testing

| Kind | Where | What |
|---|---|---|
| Unit | each `review_*` crate | model round trips, anchor mapping, patch parsing, design warnings, area splitting |
| Corpus | `tests/corpus/` | anchors, patches, designs, review JSON, against outputs recorded from the TypeScript code |
| Conformance | `tests/conformance/` | recorded MCP sessions replayed against `review_mcp`; the `co-review` bridge tests (ported `mcp-bridge.test.mjs`, `stale-desktop.test.mjs`) |
| GPUI | `review_ui` | `TestAppContext` / `VisualTestContext`: create a thread from the gutter, reply, submit, navigate; use GPUI executor timers, never `smol::Timer`, in tests |
| End to end | `tests/e2e/` | the Claude Code plugin and the Pi package run each workflow against a headless app with a scripted reviewer |
| Benchmarks | `crates/review_benchmarks` | the performance targets in `design.md` |

A feature is done when: every scenario in its spec passes, its tests are in CI, `docs/co-review/` and
`llms.txt` describe it (if the agent-facing behaviour changed, which it shouldn't), and it works on macOS, Linux
and Windows.

## 7. Working agreements

- **Commits**: one feature or fix per commit, subject in the present tense saying what is now true for the user
  ("Threads stay on their code while you type"), body with the why; mention the capability (`inline-review`). No
  AI co-author or attribution lines.
- **Upstream merges**: their own commits ("Merge Zed v0.xxx.y"), never mixed with feature work; after each, run the
  full test suite and walk `fork-surface.md`.
- **Versions**: the app version is the single source; `make check-versions` (ported) fails when the plugin
  manifests or the Pi / Oh My Pi packages differ. Releases bump only the recorded version fields, never a blind
  search-and-replace of the version string.
- **Docs**: the user docs (`README.md`, `docs/co-review/workflows.md`, `agents.md`, `agent-setup.md`,
  `design-docs.md`) are carried over and updated for anything that changed (install, the desktop-only app,
  keybindings off macOS). `docs/co-review/architecture.md` is rewritten for the crates.
- **Order of work**: follow `tasks.md`. Inside a phase, the headless crate before its UI, the test
  before the port, the conformance recording before the crate.
- **When stuck** on GPUI for more than half a day, find the Zed surface that already does something similar
  (section 4's table) and copy its shape before inventing one.
