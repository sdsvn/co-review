# Fork surface

Every edit Co-Review makes to Lapce's own files, each marked `// co-review:` (`# co-review:` in TOML), so
`git grep "co-review:"` lists them. Review this list at every upstream merge. Co-Review's own code is in new files:
`crates/review_*` and `lapce-app/src/review.rs` + `lapce-app/src/review/` (the review UI, which needs `lapce-app`'s
internal types, so it can't be a crate).

| File | Hook | Why | Upstream PR could remove it? |
|---|---|---|---|
| `lapce-app/Cargo.toml` | depends on `review_mcp`, `review_model`, `review_store` | the review core and MCP server run in the app process (design.md, decision 4) | no |
| `lapce-app/src/lib.rs` | `pub mod review;` | the review module tree | no |
| `lapce-app/src/app.rs` | `Cli::background` (`--background`) | `co-review mcp` starts the app with no window until a review is shown or the app is activated (desktop-app) | no |
| `lapce-app/src/app.rs` | `launch`: a `--background` start returns when an instance already runs | handing it a launch would focus a window | no |
| `lapce-app/src/app.rs` | `launch`: `review::ReviewCore::start` before the windows, `review::connect` after | the review core, the MCP server (`server.json`), store events as signals, window requests, window open/close for agents | a generic "app started" callback would shrink it |
| `lapce-app/src/app.rs` | `launch`: no initial window with `--background` | desktop-app: background start | no |
| `lapce-app/src/app.rs` | `OpenPaths` handler: folders go to `review::open_or_focus_folder` | one window per repository: a launch with a folder opens its window or focuses it (Lapce opens a workspace tab in the active window) | an option for "folder = window" upstream |
| `lapce-app/src/app.rs` | `AppEvent::WillTerminate`: `ReviewCore::shut_down` | quitting tells agents their review windows closed, and removes `server.json` | an "app quitting" callback |
| `lapce-app/src/app.rs` | `get_socket`, `listen_local_socket` use `review::local_socket` | one single-instance socket per `$CO_REVIEW_HOME`, so apps on different homes (tests, agents) never hand launches to each other | goes away with Co-Review's own config folder (branding) |
| `lapce-app/src/app.rs` | `editor_tab_content`: `EditorTabChild::Review` → `review::review_page_view` | review pages in editor tabs | a pluggable tab-child view |
| `lapce-app/src/editor_tab.rs` | `EditorTabChildInfo::Review`, `EditorTabChildSource::Review`, `EditorTabChild::Review(ReviewPageId, ReviewPage)` and their arms (header via `review::review_page_view_info`) | review pages (overview, document, pull request) as tab children, like Settings and Keymap | a pluggable tab-child kind |
| `lapce-app/src/main_split.rs` | `EditorTabChild::Review` / `EditorTabChildSource::Review` arms; `MainSplitData::open_review_page` | opening, finding and splitting review page tabs | same |
| `lapce-app/src/panel/kind.rs` | `PanelKind::Review` (icon, default position left) | the Review panel | a pluggable panel kind |
| `lapce-app/src/panel/view.rs` | `PanelKind::Review` → `review::review_panel`; tooltip "Review" | the Review panel's view | same |
| `lapce-app/src/panel/data.rs` | `PanelKind::Review` in `default_panel_order` (after the file explorer); `PanelSection::Reviews` | the panel in the default layout (saved layouts get it from `LapceDb::get_panel_orders`); the panel's foldable section | same |
| `lapce-app/src/window_tab.rs` | `WindowTabData::review: ReviewWorkspaceData`, built in `WindowTabData::new` | each workspace's reviews, loaded off the UI thread and kept in step with store changes | no |
| `lapce-app/src/window_tab.rs` | `ReviewTogglePanel`, `ReviewOpenOverview` handlers; `PanelKind::Review` in `toggle_panel_focus` | the `Review:` commands | no |
| `lapce-app/src/command.rs` | `LapceWorkbenchCommand::ReviewTogglePanel` ("Review: Toggle Review Panel"), `ReviewOpenOverview` ("Review: Open Review Overview") | review commands under `Review:` in the palette | no |
| `lapce-app/src/config/icon.rs`, `defaults/icon-theme.toml`, `icons/codicons/comment-discussion.svg` (new) | `LapceIcons::REVIEW` = `"review"` | the Review panel's and review pages' icon | no |
