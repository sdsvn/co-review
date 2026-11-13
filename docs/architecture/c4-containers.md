# Containers

Level 2 of the [C4 model](README.md): the separately running parts of Co-Review and how they talk. Libraries that are bundled into a part (the review model in `src/common`, Tree-sitter, Theia itself) are not containers; [the component diagram](c4-components-review-backend.md) shows the inside of the backend.

```mermaid
C4Container
  title Container diagram for Co-Review

  System_Ext(acpAgent, "ACP agent", "Launched by the backend")
  System_Ext(repo, "Git repository", "The code under review")
  System_Ext(harness, "Agent harness", "Claude Code plugin, Pi and Oh My Pi packages, any MCP client")
  System_Ext(github, "GitHub", "Pull requests")
  Person(reviewer, "Reviewer", "Reads and comments, in the window or on the phone")

  System_Boundary(coreview, "Co-Review") {
    Container(frontend, "Review workbench", "Theia frontend, Monaco, React", "The window: editor with inline threads, Review panel, documents, patch pages")
    Container(backend, "Co-Review backend", "Node.js, Theia backend", "Review state, anchors, repository map, agents, /mcp, overview pages")
    Container(cli, "co-review command", "Node.js CLI", "Opens or focuses the app; co-review mcp bridges an agent's stdio MCP to the backend")
    Container(electronMain, "Desktop shell", "Electron main process", "Owns the windows: one per repository or review; background start; single instance")
    Container(phone, "Phone view", "HTML and JavaScript at /m/", "The reviewer's pending questions and threads on a phone")
    Container(reviewServer, "co-review-server", "Node.js CLI", "Opens a review directory for scripts; the backend keeps its review.json current")
    ContainerDb(store, "Review home", "JSON files under ~/.co-review", "Reviews, snapshots, map and facts, overview pages, server.json")
  }

  Rel(reviewer, frontend, "Reads, comments, submits", "window or browser")
  Rel(harness, cli, "Runs co-review mcp", "MCP over stdio")
  Rel(frontend, backend, "Reviews, locations, agents", "JSON-RPC")
  Rel(cli, backend, "Forwards tool calls", "Streamable HTTP /mcp")
  Rel(cli, electronMain, "Starts the app in the background", "launch")
  Rel(phone, backend, "Pending decisions", "REST /api/m")
  Rel(reviewServer, backend, "Opens the review", "HTTP")
  Rel(backend, store, "Reads and writes", "JSON files")
  Rel(backend, acpAgent, "Launches, prompts", "ACP over stdio")
  Rel(backend, repo, "Reads files, git, worktrees", "file system")
  Rel(backend, github, "Posts the review", "gh CLI")

  UpdateRelStyle(reviewer, frontend, $offsetX="-30", $offsetY="-20")
  UpdateRelStyle(harness, cli, $offsetX="-40", $offsetY="-20")
  UpdateRelStyle(frontend, backend, $offsetX="-60", $offsetY="-30")
  UpdateRelStyle(cli, backend, $offsetX="-40", $offsetY="30")
  UpdateRelStyle(cli, electronMain, $offsetX="-150", $offsetY="-45")
  UpdateRelStyle(phone, backend, $offsetX="-110", $offsetY="-20")
  UpdateRelStyle(reviewServer, backend, $offsetX="-50", $offsetY="-20")
  UpdateRelStyle(backend, store, $offsetX="10", $offsetY="-20")
  UpdateRelStyle(backend, acpAgent, $offsetX="-30", $offsetY="-130")
  UpdateRelStyle(backend, repo, $offsetX="-60", $offsetY="-10")
  UpdateRelStyle(backend, github, $offsetX="30", $offsetY="-30")
  UpdateLayoutConfig($c4ShapeInRow="4", $c4BoundaryInRow="1")
```

## Reading it

- **One backend serves everything.** The workbench, the phone view, the MCP bridge and `co-review-server` all talk to the same Node.js process, so every frontend and every agent sees the same review state.
- **The desktop shell owns the windows.** It opens one per repository or review. To show a review, the backend launches the app again with the repository and `--review=<id>`; the running shell receives it as a second instance and opens or focuses that review's window. Those two arrows are left off this diagram because they would cross other boxes; [the dynamic diagram](c4-dynamic-agent-review.md) draws the second. The shell is also optional: from a checkout, `co-review` starts the browser app on `127.0.0.1:3000` and there is no Electron main process. Installed from the app, the same command runs inside the bundle and the desktop shell owns the windows; in the browser app, showing a review opens the system browser on it instead. [Running and packaging](../running-and-packaging.md) has both paths.
- **Agents never talk to the backend directly.** A harness runs `co-review mcp`; the bridge finds the backend through `server.json` in the review home, starts the app if it is not running, and forwards tool calls. The Claude Code plugin, the Pi package and the Oh My Pi package all do this.
- **The review home is outside the repository.** Reviews, snapshots and the repository map live under `~/.co-review` (`CO_REVIEW_HOME`), so a review never shows up as a change in the repository.
