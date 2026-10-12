## Context

The Theia app is one extension (about 12,800 lines of TypeScript): a Node backend (review store, MCP server, ACP
client, Tree-sitter index, Graphify runner, bundles, GitHub, phone view) and a browser frontend (inline threads in
Monaco, the Review panel, the patch page, the document page with Mermaid, the overview page). On a 3,600-file
repository the window took 38 s to appear, 34 s of it spent on Graphify and the Tree-sitter index; the window now
opens first and those sections fill in later, but the indexes are still rebuilt for every worktree.

Lapce is a Rust editor built on Floem (its own reactive UI library), Apache-2.0, small (four crates: `lapce-app`,
`lapce-core`, `lapce-proxy`, `lapce-rpc`) and slow-moving upstream. Its WASI plugins can start language servers and
processes but can't add UI, so review surfaces need a fork. It builds with Rust and the Command Line Tools (no Xcode,
no cmake) in about 10 GB.

## Goals / Non-Goals

**Goals:**

- Every behaviour in the capability specs, on macOS, Linux and Windows.
- Agents and existing users don't notice the switch: same MCP contract, same data, same command.
- Fast: the window is up in under a second; scrolling large diffs and files with many threads keeps the display's
  frame rate; nothing the reviewer sees waits on an index.
- A fork that stays cheap to keep current with upstream.

**Non-Goals:**

- A browser app (dropped for good).
- Features Co-Review doesn't have today.
- Re-implementing what Lapce provides: navigation, search, Git, language servers, symbols, terminal, settings.
- Lapce's remote development and plugin marketplace as review features (kept, not extended).

## Decisions

0. **Host editor: Lapce** (chosen 2026-10-12 over Zed: Zed needs Xcode, cmake and a ~30 GB build and is GPL-3.0;
   Lapce needs none of that). Only this file depends on the host; the proposal and specs hold for any host.
1. **Fork, not plugin.** Lapce plugins can't draw UI; threads in the editor, the Review panel and the review pages
   need Floem views inside `lapce-app`.
2. **This repository is the fork.** Lapce's history is merged at the root (`lapce-app/`, `lapce-core/`,
   `lapce-proxy/`, `lapce-rpc/`), beside `crates/review_*` in one Cargo workspace. Upstream changes arrive by merging
   `lapce/master` (remote `lapce`); it moves slowly, so conflicts are rare. The Theia app stays until the Lapce app
   replaces it.
3. **New code in new crates; few, marked hooks.** Edits to Lapce's own files are hooks marked `// co-review:` and
   listed in `docs/co-review/fork-surface.md`. Expected hooks: a `PanelKind::Review` panel (`panel/kind.rs`,
   `panel/view.rs`), review pages as new editor-tab children like Settings and Keymap (`editor_tab.rs`), gutter
   affordance and thread markers (`editor/gutter.rs`, `editor/view.rs`), changed-file decorations and a filter in the
   file explorer (`file_explorer/`), app entry, `--background` and branding (`bin/lapce.rs`, `app.rs`), and window
   close / open-or-focus by root (`window.rs`, `app.rs`). The review UI is not a crate: it lives in the new module
   tree `lapce-app/src/review/`, since it needs `lapce-app`'s internal types and a `review_ui` crate that
   `lapce-app` depends on would be a cycle.
4. **One process.** The review core and the MCP server run in the app process (not `lapce-proxy`, which serves
   remote workspaces); started with `--background` it opens no window, so `co-review mcp` can start it for an agent.
5. **Core crates know nothing about windows.** Model, store, anchors, index, graph, bundles, design check, pages,
   GitHub, code folders, MCP and web are plain crates (std threads and channels); `lapce-app/src/review/` adapts their events
   to Floem signals. They're tested headless.
6. **ACP through the `agent-client-protocol` crate** (the protocol's Rust SDK), in `review_agents`; Lapce has no ACP
   client.
7. **Mermaid ported from Ferrite** (MIT, `src/markdown/mermaid/`): its parsers and layout are kept, its egui
   drawing is redone in Floem. Drawing nodes ourselves gives the hit areas node and edge comments need.
8. **Markdown pages start from Lapce's hover renderer** (`markdown.rs`, pulldown-cmark, images), extended with
   selection-to-source mapping for text anchors, the design tree and Mermaid blocks.
9. **The pull-request page is a scrolling list of per-file diffs** built on Lapce's diff editor
   (`editor/diff.rs`), with thread rows between lines; Lapce has no multi-file diff view.
10. **MCP server with `rmcp`** (Streamable HTTP on `127.0.0.1`), plus the stdio bridge in the `co-review` command.
11. **Indexes keyed by content, per repository.** The repo map caches symbols by git blob hash in a cache shared by
    every worktree of the repository (found through `git rev-parse --git-common-dir`); the Graphify output is shared
    the same way and seeded into a new worktree before `graphify update`, which then re-extracts only changed files.
    `review_index` uses the same tree-sitter version as `lapce-core`.
12. **Same data, same contract.** serde types with the Theia app's field names; data written by either app is read
    by the other, so users can switch back.
13. **Desktop only.** No browser target; `open_review` returns a `note`.
14. **Licenses and branding.** Lapce's code stays Apache-2.0; Co-Review's crates keep the project's EPL-2.0 (the
    two combine). The app is branded Co-Review: own name, icon, bundle id `dev.co-review.app`, config folder and
    update feed; Lapce's update check points at Co-Review's releases. Lapce's plugin registry stays: language
    support (language servers) comes from Lapce plugins.
15. **Platforms**: macOS (Apple silicon), Linux (x64, arm64) and Windows (x64), as today. `CO_REVIEW_PORT` stays
    the MCP server's port.

### Crates

| Crate | Responsibility | Ported from (extensions/review/src) |
|---|---|---|
| `review_model` | reviews, threads, locations, anchors, agent payloads | `common/review-model.ts`, `review-protocol.ts` |
| `review_store` | `~/.co-review` persistence, per-review serialized mutations, change events | `node/review-store.ts`, `participants.ts` |
| `review_anchor` | tracked ranges, snapshots, mapping and search | `common/anchor-tracking.ts`, `node/anchor-tracker.ts`, `anchor-snapshots.ts` |
| `review_syntax` | symbol paths, token anchors (tree-sitter, as in `lapce-core`) | `node/syntax-service-impl.ts` |
| `review_index` | repository map, areas, coverage | `node/repo-index.ts` |
| `review_graph` | Graphify runner, blast radius, graph overview | `node/graphify.ts`, `repo-overview.ts` |
| `review_bundle` | review directories, patches, `PR.md`, sidecar findings, suggested edits, OpenSpec, `review.json` | `node/bundle-service.ts`, `review-payloads.ts`, `common/patch.ts` |
| `review_design` | design-document parsing and format warnings | `common/design-format.ts` |
| `review_pages` | overview pages | `node/overview-page.ts` |
| `review_markdown` | Markdown pages as blocks, the page's text and text anchors (as the Theia DOM has them), links, OpenSpec cards | `browser/document/document-render.ts`, `text-anchor.ts` |
| `review_mermaid` | Mermaid parsing, layout and drawing as primitives with hit areas | Ferrite's `src/markdown/mermaid/` (MIT) |
| `review_github` | posting through `gh` | `node/github.ts` |
| `review_code` | code folders, worktrees, base versions | `node/code-folders.ts` |
| `review_mcp` | MCP server, `server.json`, presence, decisions, channel | `node/co-reviewer-mcp.ts`, `agent-coordination.ts` |
| `review_agents` | ACP participation in threads | `node/acp-agent-service.ts` |
| `review_cli` | the `co-review` command | `bin/co-review.mjs`, `node/agent-setup.ts`, `debug-log.ts` |
| `lapce-app/src/review/` (module, not a crate) | Floem views: panel, inline threads, patch page, document page, Mermaid, dialogs, theme | `browser/*`, Ferrite's Mermaid |

### Performance targets

Checked by benchmarks in CI (a 3,600-file TypeScript repository, a 20,000-file Rust repository, a 5,000-line
diff); a regression over 20 % fails the build.

| What | Target |
|---|---|
| Window for a prepared review | < 1 s from launch |
| `open_review` with `open: false` | < 200 ms, never waits on an index |
| Overview page, first write | < 300 ms |
| Repo map cold, 3,600 files | < 5 s, background threads |
| Repo map in a second worktree or after restart | < 2 s |
| Patch page, 5,000 diff lines | opens < 300 ms, scrolls at frame rate |
| 50 inline threads in a 2,000-line file | scroll at frame rate |
| Reviewer message to `await_reviewer` | < 50 ms |

## Risks / Trade-offs

- [Floem's editor can't place a widget between lines; inline threads need one] → first spike: line-height
  injection or phantom lines in Floem's editor (Floem is a git dependency, so this may mean forking Floem too);
  fallback: threads as gutter markers that open a pinned popover, with full threads in the panel.
- [Lapce upstream is slow; editor bugs are ours to fix] → accepted; keep fixes small and offer them upstream.
- [Floem is young and thinly documented] → copy the shapes of Lapce's own views (settings, source control, diff).
- [Text anchors in rendered Markdown] → map the rendered selection to source offsets once, in `lapce-app/src/review/`; verify
  against the Theia app's anchors.
- [Ferrite's Mermaid layout differs from mermaid.js] → node and edge ids (what anchors use) come from the source,
  not the layout, so comments survive layout differences.
- [Behaviour drift] → recorded MCP sessions and corpora from the Theia app as conformance tests.
- [Windows] → Windows CI from the first task.
- [Browser, phone and script users lose their entry points] → the browser app, the phone view and the review
  server (`co-review-server`) are dropped (decided 2026-10-12); release notes say so.

## Migration Plan

1. Ship the content-keyed repo map and shared Graphify output in the Theia app first (same design as decision 9).
2. Build the Lapce app against the same `~/.co-review`; users can run either app on the same data.
3. Switch `install.sh` and the release workflow to the Lapce app; the integrations update as they do today.
4. Rollback: reinstall the last Theia release; the data is compatible both ways.

## Open Questions

- None blocking. The upstream base is `lapce/master` (no stable release since 0.4.6).
