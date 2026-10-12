## Context

The Theia app is one extension (about 12,800 lines of TypeScript): a Node backend (review store, MCP server, ACP
client, Tree-sitter index, Graphify runner, bundles, GitHub, phone view) and a browser frontend (inline threads in
Monaco, the Review panel, the patch page, the document page with Mermaid, the overview page). On a 3,600-file
repository the window took 38 s to appear, 34 s of it spent on Graphify and the Tree-sitter index; the window now
opens first and those sections fill in later, but the indexes are still rebuilt for every worktree.

Zed is Rust and GPUI, GPL-3.0 for the editor crates, merges upstream daily, and can't be extended with UI through
its WASM extensions.

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
- Re-implementing what Zed provides: navigation, search, Git, language servers, symbols, terminal, settings.
- Zed's collaboration, AI assistant and edit prediction (hidden, not shipped as features).

## Decisions

0. **Host editor: Zed** (pending the license question). Only this file depends on the choice; the proposal and
   specs hold for any host. Alternative: Lapce (Apache-2.0, small and slow-moving; has no ACP client, Mermaid,
   multi-file diff view or rendered Markdown pages, so `review_agents`, `review_ui` and the patch page grow by
   weeks). If the host changes, rewrite decisions 1–12 and the crate table for it; leave the specs alone.
1. **Fork, not extension.** Zed's extensions can't add panels or editor UI; threads in the editor and the Review
   panel need GPUI.
2. **Keep Zed whole; hide, don't delete.** Debugger, tasks, REPL, collab, edit prediction and the AI panel are off
   in the default settings and out of the command palette (a `review_mode` flag). Deleting crates would make every
   upstream merge conflict.
3. **New code in new crates.** Edits to Zed's crates are small hooks, each marked `// co-review:` and listed in
   `docs/co-review/fork-surface.md`. Expected hooks: app entry and branding (`zed`), a gutter affordance and
   selection toolbar entry (`editor`), entry decorations and a filter (`project_panel`), window close and
   open-or-focus by root (`workspace`), node and edge geometry (`mermaid_render`, only if the SVG lacks it), update
   feed and sign-in (`auto_update`, `client`).
4. **One process.** The review core runs in the app; started with `--background` it opens no window, so
   `co-review mcp` can start it for an agent. Alternative considered: a separate daemon (more to package and
   update, no benefit with one app).
5. **Core crates know nothing about windows.** Model, store, anchors, index, graph, bundles, design check, pages,
   GitHub, code folders, MCP and web are plain crates (GPUI tasks and entities at most); they're tested headless.
6. **ACP through Zed's `agent_servers` and `acp_thread`**, not a new client.
7. **Mermaid through Zed's `mermaid_render`.** Node and edge hit areas come from the SVG it produces.
8. **MCP server with `rmcp`** (Streamable HTTP on `127.0.0.1`), plus the stdio bridge in the `co-review` command.
   Zed's `context_server` is an MCP client, not a server.
9. **Indexes keyed by content, per repository.** The repo map caches symbols by git blob hash in a cache shared by
   every worktree of the repository (found through `git rev-parse --git-common-dir`); the Graphify output is shared
   the same way and seeded into a new worktree before `graphify update`, which then re-extracts only changed files.
10. **Same data, same contract.** serde types with the Theia app's field names; data written by either app is read
    by the other, so users can switch back.
11. **Desktop only.** No browser target; `open_review` returns a `note`.
12. **GPL-3.0-or-later and no Zed branding**: own name, icon, bundle id `dev.co-review.app`, config folder, update
    feed; no Zed sign-in or telemetry.

### Crates

| Crate | Responsibility | Ported from (extensions/review/src) |
|---|---|---|
| `review_model` | reviews, threads, locations, anchors, agent payloads | `common/review-model.ts`, `review-protocol.ts` |
| `review_store` | `~/.co-review` persistence, per-review serialized mutations, change events | `node/review-store.ts`, `participants.ts` |
| `review_anchor` | tracked ranges, snapshots, mapping and search | `common/anchor-tracking.ts`, `node/anchor-tracker.ts`, `anchor-snapshots.ts` |
| `review_syntax` | symbol paths, token anchors (Zed's grammars and outline queries) | `node/syntax-service-impl.ts` |
| `review_index` | repository map, areas, coverage | `node/repo-index.ts` |
| `review_graph` | Graphify runner, blast radius, graph overview | `node/graphify.ts`, `repo-overview.ts` |
| `review_bundle` | review directories, patches, `PR.md`, sidecar findings, suggested edits, OpenSpec, `review.json` | `node/bundle-service.ts`, `review-payloads.ts`, `common/patch.ts` |
| `review_design` | design-document parsing and format warnings | `common/design-format.ts` |
| `review_pages` | overview pages | `node/overview-page.ts` |
| `review_github` | posting through `gh` | `node/github.ts` |
| `review_code` | code folders, worktrees, base versions | `node/code-folders.ts` |
| `review_mcp` | MCP server, `server.json`, presence, decisions, channel | `node/co-reviewer-mcp.ts`, `agent-coordination.ts` |
| `review_agents` | ACP participation in threads | `node/acp-agent-service.ts` |
| `review_web` | phone view, review server API | `node/mobile-*.ts`, `bin/review-server.mjs` |
| `review_cli` | the `co-review` command | `bin/co-review.mjs`, `node/agent-setup.ts`, `debug-log.ts` |
| `review_ui` | panel, inline threads, patch page, document page, dialogs, theme | `browser/*` |

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

- [GPUI is thinly documented; text inputs inside editor blocks are hard] → spikes before porting (tasks 0.x); copy
  the shapes of `agent_ui`, `git_ui` and `diagnostics`.
- [Upstream churn] → base on Zed stable tags, merge weekly, keep the fork surface small and listed.
- [Mermaid node geometry may not survive `mermaid_render`] → parse node and edge groups from its SVG; else add a
  small API upstream.
- [Text anchors in rendered Markdown] → map the rendered selection to source offsets once, in `review_ui`; verify
  against the Theia app's anchors.
- [Behaviour drift] → recorded MCP sessions and corpora from the Theia app as conformance tests.
- [GPL-3.0, trademark] → owner sign-off; full rebranding.
- [Windows support in Zed is younger] → Windows CI from the first task.
- [Browser users lose their app] → phone view and review server remain; release notes say so.

## Migration Plan

1. Ship the content-keyed repo map and shared Graphify output in the Theia app first (same design as decision 9).
2. Build the Zed app against the same `~/.co-review`; users can run either app on the same data.
3. Switch `install.sh` and the release workflow to the Zed app; the integrations update as they do today.
4. Rollback: reinstall the last Theia release; the data is compatible both ways.

## Open Questions

- Accept GPL-3.0-or-later for the app?
- A new `co-review-zed` repository, or replace this repository's app in place?
- Zed stable tags or upstream `main` as the base (this design assumes stable tags)?
