# Building Co-Review on Lapce: instructions

For whoever builds the Lapce fork, person or agent. The OpenSpec change [`openspec/changes/native-rebuild`](openspec/changes/native-rebuild/proposal.md) says what to build
(`specs/`), why (`design.md`) and in which order (`tasks.md`); this file says how. When they disagree, the specs win
and this file is fixed.

## 1. The fork

### Repository layout

This repository is the fork. Lapce's history is merged at the root, beside Co-Review's own files, in one Cargo
workspace:

```
lapce-app/ lapce-core/ lapce-proxy/ lapce-rpc/   Lapce (upstream, Apache-2.0)
crates/review_*                                  Co-Review's crates (EPL-2.0)
integrations/ plugin/ prompts/ site/ docs/ llms.txt install.sh packaging/
```

Building needs Rust (the version in `rust-toolchain.toml`) and the platform's C toolchain: the Command Line Tools on
macOS (`xcode-select --install`), `build-essential` and the usual X11/Wayland headers on Linux, MSVC on Windows. No
Xcode, no cmake. `cargo build` builds Lapce and the review crates; `cargo run --bin lapce` runs the app.

### Upstream

```bash
git fetch lapce
git merge lapce/master
```

- The remote `lapce` is https://github.com/lapce/lapce. There are no recent releases, so the base is `master`.
- Merge, never rebase: the history must show what came from upstream. Upstream merges are their own commits
  ("Merge Lapce <short sha>"), never mixed with feature work.
- Lapce's `README.md`, `CONTRIBUTING.md`, `Makefile` and docs live under `docs/lapce/` and `Makefile.lapce`. On a merge, take their
  changes there, not at the root. Lapce's release workflow is not kept: Co-Review's `.github/workflows/release.yml`
  and `packaging/` build and package the app, so a merge that brings Lapce's release workflow back drops it again.

### The fork's edit budget

- **New code goes in new crates** named `review_*` (the crate table is in `design.md`). The one exception is the
  review UI: it lives in the module tree `lapce-app/src/review/` (new files only), because it needs `lapce-app`'s
  internal types and a separate crate would form a dependency cycle with `lapce-app`.
- **An edit to a Lapce file is an extension point, not a feature.** Add the smallest hook that lets a `review_*`
  crate do the work: an enum variant, a callback, a `pub` on a private item. Never write review logic inside
  `lapce-*`.
- **Mark every edit** to a Lapce file with a `// co-review:` comment saying why the hook is needed, so
  `git grep "co-review:"` lists the whole fork surface.
- **Keep the list in `docs/co-review/fork-surface.md`**: file, hook, why, and whether an upstream PR could make it
  unnecessary. Review it at every upstream merge.
- **Expected hooks** (anything beyond these needs a decision in `design.md` first):

| Lapce file | Hook | For |
|---|---|---|
| `lapce-app/src/panel/kind.rs`, `panel/view.rs` | `PanelKind::Review` and its view | review-panel |
| `lapce-app/src/editor_tab.rs` | review pages (overview, document, pull request) as tab children, like `Settings` and `Keymap` | overview-page, document-review, change-review |
| `lapce-app/src/editor/gutter.rs`, `editor/view.rs` | the **+** affordance, thread markers, thread rows between lines | inline-review |
| `lapce-app/src/file_explorer/` | entry decorations (colour, `+12 −3`, viewed mark) and a changed-files filter | change-review, review-panel |
| `lapce-app/src/bin/lapce.rs`, `app.rs`, `window.rs` | starting the review core, `--background`, open-or-focus by root, window close, branding | desktop-app |
| `lapce-app/src/window_tab.rs`, `main_split.rs`, `command.rs`, `config/icon.rs` | each workspace's review data, opening review page tabs, the `Review:` commands, the review icon | review-panel, overview-page |
| `lapce-app/src/update.rs`, `plugin.rs` | Co-Review's update feed; the plugin registry kept or disabled | desktop-app |

### Branding

- Product name **Co-Review**, binary `co-review`, bundle id `dev.co-review.app`, data in `~/.co-review`, and its own
  config folder so it never shares settings with an installed Lapce.
- Remove Lapce's names, logos and URLs from everything user-visible; keep the copyright and license notices
  Apache-2.0 requires.

## 2. Compatibility contract

Agents and existing users must not notice the switch. These are fixed and covered by tests; change them only
together with every integration and with a version bump.

1. **MCP tools**: names, arguments, defaults, result shapes, error texts and the server instructions, exactly as in
   `llms.txt` and the MCP sessions recorded from the Theia app (`crates/review_mcp/tests/conformance/`).
2. **Data**: `~/.co-review` layout (`workspaces/<sha256(root uri)[0:16]>/reviews/*.json`, `workspace.json`,
   `snapshots/`, `pages/`, `worktrees/`, `server.json`, `logs/`), `CO_REVIEW_HOME`, and every JSON shape in
   `review_model` (the Theia app's `review-model.ts`). The new app reads data the Theia app wrote, and the other way round, so users can switch back.
3. **Review directories**: `review.json`, `PR.md`, `<patch>.comments.json(l)` and the anchor formats in `llms.txt`.
4. **The `co-review` command**: subcommands, flags, exit codes and the stdio MCP bridge's behaviour (handshake
   answered locally, app started on the first tool call, reconnect, exit with the harness).
5. **One deliberate change**: there is no browser app (`design.md`, decision 13). `open_review` returns a `note` and
   never a `url`; the prompts, skills and `llms.txt` say to tell the reviewer the review is open in Co-Review.
6. **Prompts**: `prompts/` stays the single source; `bin/gen-prompts.mjs` copies them into skills, commands, agents,
   app text and docs. Never edit a generated copy.

**How it's checked.** The reference behaviour was recorded from the Theia app before each crate was written. The
Theia app and its recorders are gone (task 8.4); the recorded fixtures are now the reference: each crate's
`tests/fixtures/` (and `review_index`'s `tests/gen/expected.json`), replayed by its tests, and the MCP sessions for
the four workflows in `crates/review_mcp/tests/conformance/`. Change a fixture only for a deliberate contract change,
and say so in the commit.

## 3. Where the TypeScript went

The Theia app has been removed; `git log -- extensions/` has it. Behaviour was ported, not structure: idiomatic Rust,
checked against the recorded fixtures; workarounds for Theia (DI, RPC between frontend and backend, Monaco view
zones) were dropped. Where each part of the Theia app went:

| Theia app (`extensions/review/src`, removed) | Crate | Notes |
|---|---|---|
| `common/review-model.ts`, `review-protocol.ts` | `review_model` | serde with the same field names; unknown fields kept |
| `common/anchor-tracking.ts`, `node/anchor-tracker.ts`, `anchor-snapshots.ts` | `review_anchor`, `review_store` | diff-match-patch ported exactly; offsets in UTF-16 code units |
| `node/syntax-service-impl.ts`, `browser/syntax-symbols.ts` | `review_syntax` | native tree-sitter, the same version as `lapce-core` |
| `node/review-store.ts`, `participants.ts` | `review_store` | one lock per review; change events on a channel |
| `node/repo-index.ts` | `review_index` | symbols cached by **git blob hash**, shared per repository |
| `node/graphify.ts`, `repo-overview.ts` | `review_graph` | one Graphify output per repository |
| `node/overview-page.ts` | `review_pages` | cheap parts first, slow sections later |
| `node/bundle-service.ts`, `review-payloads.ts`, `common/patch.ts` | `review_bundle` | |
| `common/design-format.ts` | `review_design` | |
| `node/github.ts` | `review_github` | shells out to `gh` |
| `node/code-folders.ts` | `review_code` | git worktrees, sparse checkout, base file reads |
| `node/co-reviewer-mcp.ts`, `agent-coordination.ts` | `review_mcp` | `rmcp`, Streamable HTTP; channel push for Claude Code |
| `node/acp-agent-service.ts`, `agent-setup.ts` (agent list) | `review_agents` | the `agent-client-protocol` crate |
| `bin/co-review.mjs`, `node/agent-setup.ts` (setup), `debug-log.ts` | `review_cli` | starts fast: no UI crates on the `mcp` path |
| `browser/*` (panel, editor decorator, inline zone, document and patch widgets, theme) | `lapce-app/src/review/` | Floem views, see section 4 |
| `electron-main/*` | Lapce hooks + `lapce-app/src/review/` | `--background`, single instance, window lifecycle |

Mermaid comes from Ferrite (https://github.com/OlaProeis/Ferrite, MIT, `src/markdown/mermaid/`): keep its parsers and
layout, redo its egui drawing in Floem, and keep its license notice with the ported files.

## 4. Rust and Floem conventions

- No `unwrap()` or panicking indexing outside tests (the review crates deny `clippy::unwrap_used`); propagate errors
  with `?`; never discard a `Result` silently (log it); errors that matter to the reviewer reach the UI as a message.
- Library roots named after the crate (`[lib] path = "src/review_store.rs"`), no `mod.rs` files.
- Full-word names; comments only for a non-obvious *why*.
- Floem is reactive: state lives in signals (`RwSignal`, `create_rw_signal`), views derive from them. The review
  crates stay UI-free; `lapce-app/src/review/` turns store events (channels) into signal updates on the UI thread, never
  blocking it.
- Look like Lapce: reuse its theme colours (`LapceColor`), icons (`LapceIcons`), config and keymap system; review
  commands appear under `Review:` in the palette; keybindings as today (`Cmd+Alt+M`, `Cmd+Alt+A`, `Cmd+Alt+↓/↑`,
  `Cmd+Alt+O`, `Cmd+Alt+V`; `Ctrl` instead of `Cmd` off macOS).

**Learn from the closest Lapce code before writing a surface:**

| Building | Read first |
|---|---|
| Inline threads in the editor | `editor/view.rs`, `editor/gutter.rs`, phantom text in `doc.rs` (inlay hints, error lens) |
| Pull-request page | `editor/diff.rs` (the diff editor), `panel/source_control_view.rs` |
| Review panel | `panel/source_control_view.rs`, `panel/problem_view.rs` |
| Review pages in tabs | `editor_tab.rs` (`Settings`, `Keymap`), `settings.rs` |
| Rendered Markdown | `markdown.rs` and how `hover.rs` / `plugin.rs` display it |
| Pickers (Go to Comment, review switcher, agent picker) | `palette.rs`, `palette/kind.rs` |
| File explorer decorations | `file_explorer/` (git status colouring) |

## 5. Performance rules

1. **Never block the UI thread.** File reads, git, tree-sitter parsing, diffing, JSON of a whole review, Graphify:
   always on a background thread. Anything over 100 ms on the UI thread is logged with what it was doing.
2. **Never make the reviewer wait on an index.** Show what's cheap now; fill in slow sections when ready.
3. **Build once, reuse everywhere.** Caches are per repository and keyed by content, shared by every worktree, and
   persisted.
4. **Render only what's on screen.** Thread rows, the pull-request page and the panel use virtual lists
   (`virtual_stack`); a folded thread is one line.
5. **Incremental updates.** A store change updates the threads it touched, not the whole panel or page.
6. **Measure.** Every performance target in `design.md` gets a benchmark, run in CI; a regression of more than 20 %
   fails the build.

## 6. Testing

| Kind | Where | What |
|---|---|---|
| Unit | each `review_*` crate | what fixtures don't already cover |
| Fixtures | `crates/*/tests/fixtures` | outputs recorded from the TypeScript (now removed), replayed exactly |
| Conformance | `crates/review_mcp/tests/conformance/` | recorded MCP sessions; the `co-review` bridge tests |
| UI | `lapce-app/src/review/` | Floem views driven headless where possible; the rest checked by running the app |
| End to end | `tests/e2e/` | the Claude Code plugin and the Pi package run each workflow against the app with a scripted reviewer |

A feature is done when every scenario in its spec passes, its tests are in CI (`.github/workflows/rust.yml`), and it
works on macOS, Linux and Windows.

## 7. Working agreements

- **Commits**: one feature or fix per commit, subject in the present tense saying what is now true for the user
  ("Threads stay on their code while you type"), body with the why; mention the capability (`inline-review`). No
  AI co-author or attribution lines.
- **Versions**: the app version is the single source; `make check-versions` fails when the plugin manifests or the
  Pi / Oh My Pi packages differ. Releases bump only the recorded version fields, never a blind search-and-replace.
- **Docs**: the user docs (`README.md`, `docs/workflows.md`, `agents.md`, `agent-setup.md`, `design-docs.md`) are
  updated for anything that changed. `docs/architecture.md` describes the fork and the crates.
- **Order of work**: follow `tasks.md`. Inside a phase, the headless crate before its UI.
- **When stuck** on Floem for more than half a day, find the Lapce view that already does something similar
  (section 4's table) and copy its shape before inventing one.
