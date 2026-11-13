# An agent opens a review and answers in it

A dynamic [C4](README.md) diagram of the central flow: the agent that did the work opens a review from its own harness, the reviewer comments, the agent answers in the thread, and the review ends with a verdict. This is what `/co-review:review`, `/co-review:pr` and `/co-review:audit` all do underneath ([the workflows](../workflows.md)).

```mermaid
C4Dynamic
  title Dynamic diagram: an MCP agent reviews with the human

  Person(reviewer, "Reviewer", "Reads and comments")
  System_Ext(agent, "Agent harness", "Claude Code, Pi, Oh My Pi or another MCP client: the agent that did the work")
  Container(cli, "co-review command", "Node.js CLI", "stdio MCP bridge")
  Container(frontend, "Review workbench", "Theia frontend", "The window")
  Container(electronMain, "Desktop shell", "Electron main process", "Windows")

  Container_Boundary(backend, "Co-Review backend") {
    Component(mcp, "CoReviewerMcp", "MCP server", "The /mcp endpoint")
    Component(reviewStore, "ReviewStore", "Serialized writer", "Review state")
    Component(reviewService, "ReviewServiceImpl", "JSON-RPC service", "The window's service")
  }

  Rel(agent, cli, "open_review", "MCP over stdio")
  Rel(cli, mcp, "Forwards, starts the app if needed", "Streamable HTTP")
  Rel(mcp, reviewStore, "Creates the review with overview and findings")
  Rel(mcp, electronMain, "Shows the review: launches the app with --review", "second instance")
  Rel(electronMain, frontend, "Opens or focuses a window")
  Rel(reviewer, frontend, "Comments; at the end, submits with a verdict")
  Rel(frontend, reviewService, "Adds the thread", "JSON-RPC")
  Rel(reviewService, reviewStore, "Saves and broadcasts")
  Rel(mcp, agent, "await_reviewer returns the thread, later the verdict", "MCP")
  Rel(agent, mcp, "reply in the thread", "MCP")

  UpdateRelStyle(agent, cli, $offsetY="-30")
  UpdateRelStyle(cli, mcp, $offsetX="40", $offsetY="-20")
  UpdateRelStyle(mcp, reviewStore, $offsetY="-40")
  UpdateRelStyle(mcp, electronMain, $offsetX="-120", $offsetY="-20")
  UpdateRelStyle(electronMain, frontend, $offsetX="-20", $offsetY="-60")
  UpdateRelStyle(reviewer, frontend, $offsetX="-110", $offsetY="-50")
  UpdateRelStyle(frontend, reviewService, $offsetX="40", $offsetY="-20")
  UpdateRelStyle(reviewService, reviewStore, $offsetY="30")
  UpdateRelStyle(mcp, agent, $offsetX="-230", $offsetY="60")
  UpdateRelStyle(agent, mcp, $offsetX="30", $offsetY="30")
  UpdateLayoutConfig($c4ShapeInRow="3", $c4BoundaryInRow="1")
```

## Reading it

- **Waiting costs nothing.** After step 8 the agent is blocked in `await_reviewer`, which returns only when a thread changes or the reviewer submits (step 9). In Claude Code with the channel on, the thread arrives as a pushed message instead, and the agent answers with `reply` on it (step 10). Steps 6 to 10 repeat until the reviewer submits; then `await_reviewer` returns the verdict and the agent acts on it.
- **The window is the agent's.** In an app started in the background, when the last agent using a window it showed is done (the review was submitted, or the agent's MCP session ended), the window says so and closes after a countdown unless the reviewer keeps it open. Closing the window tells the agent the review is closed.
- **Nothing in this flow is Claude-specific.** The Pi and Oh My Pi packages run the same bridge and the same tools under their own names; any MCP client can do the same.
