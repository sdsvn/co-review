# Architecture

Co-Review is a Theia product, not a fork. Theia provides the IDE (editor, explorer, search, Git via `vscode.git`, terminal, LSP via VS Code built-ins). One Theia extension adds the review.

## Directory layout

```
applications/browser   Theia browser app (composition only)
applications/electron  Theia desktop app + electron-builder config
extensions/review      @co-review/review
  src/common           Review model and RPC protocols
  src/node             Backend: persistence, Tree-sitter,
                       ACP client, MCP endpoint
  src/electron-node    Desktop-only backend bindings
  src/electron-main    Desktop app windows (background start,
                       one window per repository)
  src/browser          Frontend: review panel, inline editor
                       UI, rendered Markdown, commands
bin/co-review.mjs      CLI: start the app, `mcp` stdio bridge
bin/gen-prompts.mjs    copies prompts/ into skills, commands,
                       agents, app text and docs
prompts/               agent prompts, written once
bin/review-server.mjs  Review server mode (co-review-server)
integrations/shared    Pi / Oh My Pi extension and slash
                       commands, shared by both packages
integrations/pi        Pi package: adapter, subagent
integrations/omp       Oh My Pi package: adapter, task agent
plugin/                Claude Code plugin: MCP server + channel,
                       skills, commands, subagent, SessionStart
                       hook (.claude-plugin/ is its marketplace)
```

The common layer defines the review model: `Review`, `ReviewThread`, `CodeLocation`, and the RPC protocols between frontend and backend.

## Backend (`src/node`)

Each backend service handles one concern.

- **`ReviewStore`** — persists reviews as JSON files under `~/.co-review/workspaces/<hash>/reviews/`. All mutations are serialized per review and broadcast, so every frontend and agent sees the same state.
- **`ReviewServiceImpl`** — JSON-RPC service, one per frontend connection (Theia `RpcConnectionHandler`).
- **`SyntaxServiceImpl`** — Tree-sitter integration (`@vscode/tree-sitter-wasm`) for symbol paths and token anchors.
- **`AcpAgentService`** — ACP client. Launches the review's agent, one session per thread. Streams answers and tool activity into threads, and routes permission requests to the reviewer.
- **`CoReviewerMcp`** — the `/mcp` endpoint (Streamable HTTP) for agents in their own harness. Registers the running instance in `~/.co-review/server.json`. For Claude Code clients it also acts as a channel (`claude/channel`): questions and submissions are pushed into the session via `startChannel`.
- **`BundleService`** — review directories: document, patches, `PR.md`, sidecar findings (proposed, idempotent), suggested edits (document rewrite / patch hand-off), OpenSpec detection, and the `review.json` state file. The companion module **`review-payloads`** maps threads to the agent-facing shapes (`t<N>` ids, targets, raw anchors).
- **`RepoIndex`** + **`graphify`** — the repository map (Tree-sitter outline per file, cached under `~/.co-review`), coverage per area, and the overview page (`repo-overview.ts`). When the `graphify` command is installed, `graphify.ts` builds or refreshes the repository's Graphify graph (`graphify update`, code only, no LLM) before the overview is built from it, and keeps a `graphify-out/` it created out of `git status` via `.git/info/exclude`.
- **`GitHubReviews`** (`github.ts`) — posts a pull request's review to GitHub (`gh api …/pulls/<n>/reviews`): line comments and suggestions from the open threads the reviewer stands behind, the rest in the body, the verdict as the event, with fallbacks for your own pull request and for lines no longer in the diff. The target comes from the review directory's `PR.md`.
- **`CodeFolders`** (`code-folders.ts`) — a change review's code (`bundle.code`), so the IDE works around the diff: the repository itself for the reviewer's own change, else a git worktree of the head under `~/.co-review/worktrees/`, checked out neighbourhood first (the changed files' folders and those of its blast radius, sparse), the rest in the background; removed when the review is archived or deleted, recreated on unarchive. The review stays keyed by its review directory; its window opens on the code folder, which lists it (`ReviewStore.listForWindow`). It also reads base versions (`readBaseFile`) and turns an editor comment on a line of the diff into a comment on the diff (`onDiff`). `graphify.ts`'s `blastRadius` gives what the change touches and what depends on it, from the Graphify graph.
- **`OverviewPages`** (`overview-page.ts`) — a review's overview page, its front page, for every review but a design or knowledge bundle (which opens on its document): the pull request (`PR.md`), the agent's overview (`open_review`'s `overview`: what it does, a Mermaid flowchart, the blast radius), the files each patch changes, every thread grouped by area, and for a repository review the repository's areas (`repo-overview.ts`). Written as Markdown under `~/.co-review/workspaces/<hash>/pages/<review>/overview.md` and rewritten when the review changes. It opens in the document view; comments on it reach agents as `target: "overview"`.
- **`MobileReview`** — the phone view (`/m/`) and its small REST API (`/api/m`). Allowed hosts: localhost plus any listed in `CO_REVIEW_ALLOWED_HOSTS`.
- **`AgentSetup`** — the in-app setup commands. Installs the bundled skills and writes (or hands out) each harness's MCP config, launching `co-review mcp` with this machine's paths.
- **`HumanDecisions` / `AgentPresenceTracker`** — pending reviewer decisions (ACP permissions, `ask_reviewer`) and whether an MCP agent is currently listening.

## Desktop windows (`src/electron-main`)

**`ReviewElectronMainApplication`** replaces Theia's Electron main application. Started with `--background` (as `co-review mcp` does for an agent), it opens no window until a review is shown or the app is activated. `open_review` shows a review by launching the app again with the repository; the running instance gets it as a second instance and opens that repository's window, or focuses it if one is open, instead of opening a second one. **`ReviewWindows`** (backend) knows which workspaces have a window (each frontend says which one it shows) and which windows agents showed. When the reviewer closes a review's window, or quits (the backend's `onStop` records it before the process exits), the review is marked `closed` and its agents' `await_reviewer` returns that, also after Co-Review restarts. In an app started with `--background`, when the last agent using a window it showed is done with it (the reviewer approved the review, or the agent's MCP session ended), the window says why and closes after a countdown unless the reviewer keeps it open; with no window left, the app quits.

## Frontend (`src/browser`)

The frontend renders the review UI in the Theia workbench.

- **`ReviewManager`** — client-side state of the current workspace: reviews, active review, and drafts.
- **`ReviewEditorDecorator`** + **`InlineZone`** — GitHub-style inline review. Click **+** in the gutter (or drag over several lines) to start a thread. Threads and drafts render under the code as Monaco view zones with overlay widgets, plus gutter glyphs and hovers.
- **`ReviewWidget`** — the Review panel: agent, drafts, and threads grouped by location.
- **`ReviewLocations`** — creates and resolves semantic locations.
- **`DocumentReviewWidget`** (`document/`) — rendered Markdown, pseudocode tree, and Mermaid with comments on text, diagrams, nodes, edges, and steps. **`PatchReviewWidget`** (`patch/`) renders patch pages with line, range, file, and patch comments. Markdown files open rendered through the document open handler; opening at a line goes to the editor. HTML is not rendered: **Open in Browser** hands the page to the system browser (`ReviewService.openInBrowser`).
- **`CodeNavigation`** (`review-code.ts`) + **`ReviewChange`** (`review-change.ts`) — stepping from a change review's diff into its code: a line number opens the head file (or, old side, the base version via the read-only `co-review-base:` resource), Cmd/Ctrl-click and hover ask the language server at the same position in the head file (loaded as a model, no editor), **Compare** opens a diff editor. `ReviewChange` reads the patch's files; `ReviewChangeOpenHandler` opens a modified file (from the explorer, Quick Open) as base ↔ head, `ReviewChangeDecorator` marks added and removed lines on a changed file opened as itself, `ReviewNavigatorDecorator` colours the changed files and folders in the explorer, and `ReviewNavigatorFilter` (replacing Theia's `FileNavigatorFilter`) narrows it to them on request. A thread on a file of the diff is labelled, grouped and opened by that file (`ReviewManager.patchFile`, `ReviewNavigator.openChangedFile`). Comments live on the head file, so they show on a diff editor's head side too. Diff threads on new-side lines are also placed in the head file's editor (never saved from there).
- **`SyntaxSymbols`** — Tree-sitter document symbols for Monaco (breadcrumbs, sticky scroll, Go to Symbol, outline) for languages with a grammar, when no language server provides symbols.
- **`ReviewShellFilter`** — removes IDE-only workbench parts (debug, tests, tasks).
- **`ReviewSelectionActions`** — the *Ask Agent / Comment* toolbar on selections.
- **`ReviewContribution`** — commands, menus, keybindings, review and agent pickers.
- **`CoReviewThemeContribution`** (`theme/`) — the Co-Review Dark / Light color themes (following the OS by default), the `coReview.agent` theme color, and the favicon. Brand assets live at `docs/assets/logo.svg` and `applications/electron/resources/icon.png` (the app icon, the logo on a 1024 px grid).

## Locations that follow the code

A comment tracks what happened to the code it is on, and says so when that code is gone. Line, range and document text comments carry a **tracked range** (`location.tracked`, `common/anchor-tracking.ts`): character offsets in a version of the file (a content hash), the text and 40 characters of context on each side, and a state, separate from the thread's status:

- `active` — the text is there unchanged (it may have moved);
- `modified` — it changed but is still identified (the original quote is kept);
- `removed` — deleted, or not located reliably;
- `ambiguous` — several places fit about equally well.

Removed and ambiguous comments are **out of scope**: never placed on code, shown in the panel's Out of scope tab, and back in scope if the code changes again and they can be placed.

**`AnchorTracker`** (backend) resolves a review's anchors whenever it is read (the window, `get_review`, `await_reviewer`) or a commented file changes: from the snapshot of the anchor's version (content-addressed under `~/.co-review/workspaces/<hash>/snapshots/`), it computes the edits to the current file with **diff-match-patch** (a line diff, then characters within changed lines) and maps the range through them with **CodeMirror change sets** (`@codemirror/state`), which also say how much of it was deleted. Same text at the mapped range: active; mostly kept and similar: modified, unless the unchanged text sits, with its context, where the edits wrote (a moved block): active there. Otherwise it searches: exact occurrences scored by matching context (one clear best: active; several close: ambiguous), then a fuzzy match (diff-match-patch's matcher) near the mapped position, only ever in text the edits wrote, never on code that was already there untouched; else removed. An edit Co-Review applies itself (an accepted suggestion) is mapped exactly, without a diff. The thresholds were tuned on real history with `extensions/review/test/anchor-corpus.mjs` (`make test` runs the unit tests).

In an open editor, before a save, the older resolution below still places comments live:

1. **Symbol** — the comment targets a symbol; resolve it by path.
2. **Exact** — the text at the stored range is unchanged.
3. **Moved** (token match) — the same token sequence appears elsewhere, surviving moves and reformatting. Persisted when the file is saved.
4. **Moved** (text match, no grammar) — for languages without a Tree-sitter grammar, exact text match (neighbours break ties), then whitespace-normalised.
5. **Outdated** — shown where it was with its original code, never moved onto code it may not describe.

Tree-sitter is used for what makes locations durable, and for a file's structure (symbols) where no language server provides it. Navigation (definition, references, hierarchies) stays with the language servers.
