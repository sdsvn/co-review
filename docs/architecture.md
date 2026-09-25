# Architecture

Co-Review is a Theia product, not a fork: Theia provides the IDE (editor, explorer, search,
Git via `vscode.git`, terminal, LSP via VS Code built-ins); one Theia extension adds the review.

```
applications/browser   Theia browser app (composition only)
applications/electron  Theia desktop app + electron-builder config
extensions/review      @co-review/review
  src/common           review model (Review, ReviewThread, CodeLocation, …) and RPC protocols
  src/node             backend: persistence, Tree-sitter, ACP client, MCP endpoint
  src/browser          frontend: review panel, inline editor UI, commands
bin/co-review.mjs      CLI: start the app, `mcp` stdio bridge for agent harnesses
bin/review-server.mjs  review server mode (`co-review-server`): open a review directory from a script
integrations/pi        Pi package: co-review extension + co-reviewer subagent
```

## Backend (`src/node`)

- **`ReviewStore`** — reviews as JSON files under `~/.co-review/workspaces/<hash>/reviews/`.
  All mutations are serialized per review and broadcast; every frontend and agent sees the same state.
- **`ReviewServiceImpl`** — JSON-RPC service per frontend connection (Theia `RpcConnectionHandler`).
- **`SyntaxServiceImpl`** — Tree-sitter (`@vscode/tree-sitter-wasm`): symbol paths and token anchors.
- **`AcpAgentService`** — ACP client: launches the review's agent, one session per thread,
  streams answers/tool activity into threads, routes permission requests to the reviewer.
- **`CoReviewerMcp`** — `/mcp` (Streamable HTTP) for agents in their own harness; registers the
  running instance in `~/.co-review/server.json`.
- **`BundleService`** — review directories: document, patches, `PR.md`, sidecar findings
  (proposed, idempotent), suggested edits (document rewrite / patch hand-off), OpenSpec detection, and the `review.json` state file; **`review-payloads`** maps threads to the agent-facing shapes (`t<N>` ids, targets, raw anchors).
- **`MobileReview`** — the phone view (`/m/`) and its small REST API (`/api/m`), local hosts plus `CO_REVIEW_ALLOWED_HOSTS`.
- **`AgentSetup`** — the in-app setup commands: installs the bundled skills and writes (or hands out) each harness's
  MCP config, launching `co-review mcp` with this machine's paths.
- **`HumanDecisions` / `AgentPresenceTracker`** — pending reviewer decisions (ACP permissions,
  `ask_reviewer`) and whether an MCP agent is currently listening.

## Frontend (`src/browser`)

- **`ReviewManager`** — client-side state of the current workspace (reviews, active review, drafts).
- **`ReviewEditorDecorator`** + **`InlineZone`** — GitHub-style inline review: `+` in the gutter
  (click or drag), threads and drafts rendered under the code (Monaco view zone + overlay widget),
  gutter glyphs and hovers.
- **`ReviewWidget`** — the Review panel: agent, drafts, threads grouped by location.
- **`ReviewLocations`** — creates and resolves semantic locations.
- **`DocumentReviewWidget`** (`document/`) — rendered Markdown / pseudocode tree / Mermaid with comments on text,
  diagrams, nodes, edges and steps; **`PatchReviewWidget`** (`patch/`) — patch pages with line/range/file/patch comments.
- **`ReviewShellFilter`** — removes IDE-only workbench parts (debug, tests, tasks).
- **`ReviewSelectionActions`** — *Ask Agent · Comment* toolbar on selections.
- **`ReviewContribution`** — commands, menus, keybindings, review and agent pickers.
- **`CoReviewThemeContribution`** (`theme/`) — the Co-Review Dark / Light color themes (the default, following the OS),
  the `coReview.agent` theme color, and the favicon. Brand assets: `docs/assets/logo.svg`,
  `applications/electron/resources/icon.png` (the app icon, the logo on a 1024 px grid).

## Locations that follow the code

A thread stores `uri`, `range`, `symbol` (e.g. `OrderService.CreateOrder`) and an anchor: the
covered text, its neighbouring lines and the Tree-sitter tokens of the range. Resolution:

1. symbol comments → the symbol, by path;
2. text unchanged at the stored range → **exact**;
3. the same token sequence elsewhere (survives moves and reformatting) → **moved**, persisted when the file is saved;
4. languages without a grammar: exact text match (neighbours break ties), then whitespace-normalised → **moved**;
5. otherwise → **outdated**: shown where it was with its original code, never moved onto code it may not describe.

Tree-sitter is only used for what makes locations durable; navigation (definition, references,
hierarchies) stays with the language servers.
