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
  src/browser          Frontend: review panel, inline editor
                       UI, commands
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
- **`MobileReview`** — the phone view (`/m/`) and its small REST API (`/api/m`). Allowed hosts: localhost plus any listed in `CO_REVIEW_ALLOWED_HOSTS`.
- **`AgentSetup`** — the in-app setup commands. Installs the bundled skills and writes (or hands out) each harness's MCP config, launching `co-review mcp` with this machine's paths.
- **`HumanDecisions` / `AgentPresenceTracker`** — pending reviewer decisions (ACP permissions, `ask_reviewer`) and whether an MCP agent is currently listening.

## Frontend (`src/browser`)

The frontend renders the review UI in the Theia workbench.

- **`ReviewManager`** — client-side state of the current workspace: reviews, active review, and drafts.
- **`ReviewEditorDecorator`** + **`InlineZone`** — GitHub-style inline review. Click **+** in the gutter (or drag over several lines) to start a thread. Threads and drafts render under the code as Monaco view zones with overlay widgets, plus gutter glyphs and hovers.
- **`ReviewWidget`** — the Review panel: agent, drafts, and threads grouped by location.
- **`ReviewLocations`** — creates and resolves semantic locations.
- **`DocumentReviewWidget`** (`document/`) — rendered Markdown, pseudocode tree, and Mermaid with comments on text, diagrams, nodes, edges, and steps. **`PatchReviewWidget`** (`patch/`) renders patch pages with line, range, file, and patch comments.
- **`ReviewShellFilter`** — removes IDE-only workbench parts (debug, tests, tasks).
- **`ReviewSelectionActions`** — the *Ask Agent / Comment* toolbar on selections.
- **`ReviewContribution`** — commands, menus, keybindings, review and agent pickers.
- **`CoReviewThemeContribution`** (`theme/`) — the Co-Review Dark / Light color themes (following the OS by default), the `coReview.agent` theme color, and the favicon. Brand assets live at `docs/assets/logo.svg` and `applications/electron/resources/icon.png` (the app icon, the logo on a 1024 px grid).

## Locations that follow the code

Comments stay attached to the code they describe, even as the file changes. Each thread stores four anchors: `uri`, `range`, `symbol` (e.g. `OrderService.CreateOrder`), and a token anchor (the covered text, its neighbouring lines, and the Tree-sitter tokens of the range).

Resolution proceeds in order:

1. **Symbol** — the comment targets a symbol; resolve it by path.
2. **Exact** — the text at the stored range is unchanged.
3. **Moved** (token match) — the same token sequence appears elsewhere, surviving moves and reformatting. Persisted when the file is saved.
4. **Moved** (text match, no grammar) — for languages without a Tree-sitter grammar, exact text match (neighbours break ties), then whitespace-normalised.
5. **Outdated** — shown where it was with its original code, never moved onto code it may not describe.

Tree-sitter is only used for what makes locations durable. Navigation (definition, references, hierarchies) stays with the language servers.
