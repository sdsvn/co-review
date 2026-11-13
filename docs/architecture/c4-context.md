# System context

Level 1 of the [C4 model](README.md): Co-Review, who uses it, and the systems around it. Everything inside Co-Review is one box here; [the container diagram](c4-containers.md) opens it.

```mermaid
C4Context
  title System context diagram for Co-Review

  Person(reviewer, "Reviewer", "Reads a repository, a change or a design and comments on it")
  System_Ext(harness, "Agent harness", "Claude Code, Pi, Oh My Pi, Codex, Cursor or any MCP client: the agent that did the work")

  System(coreview, "Co-Review", "Review tool built on Eclipse Theia: inline threads, review panel, overview pages, agent participants")

  System_Ext(acpAgent, "ACP agent", "Claude Code, Gemini CLI, OpenCode, Goose, Pi: launched by Co-Review on demand")
  System_Ext(repo, "Git repository", "The code under review, its history and its worktrees")
  System_Ext(github, "GitHub", "Pull requests the reviewer posts a review to")
  System_Ext(graphify, "Graphify", "Optional command that builds a knowledge graph of the code")
  System_Ext(langServers, "Language servers", "VS Code built-ins and gopls")

  Rel(reviewer, coreview, "Comments, asks, submits", "app, browser, phone")
  Rel(harness, coreview, "Opens reviews, answers threads", "MCP")
  Rel(coreview, acpAgent, "Launches, prompts", "ACP")
  Rel(coreview, repo, "Reads files and history", "git")
  Rel(coreview, github, "Posts the review", "gh")
  Rel(coreview, graphify, "Builds the graph", "CLI")
  Rel(coreview, langServers, "Navigation, symbols", "LSP")

  UpdateRelStyle(reviewer, coreview, $offsetX="-60", $offsetY="30")
  UpdateRelStyle(harness, coreview, $offsetX="-40", $offsetY="-70")
  UpdateRelStyle(coreview, acpAgent, $offsetX="-120", $offsetY="-20")
  UpdateRelStyle(coreview, repo, $offsetX="-40", $offsetY="10")
  UpdateRelStyle(coreview, github, $offsetX="10", $offsetY="40")
  UpdateRelStyle(coreview, graphify, $offsetX="-80", $offsetY="70")
  UpdateRelStyle(coreview, langServers, $offsetX="50", $offsetY="80")
  UpdateLayoutConfig($c4ShapeInRow="3", $c4BoundaryInRow="1")
```

## Reading it

- **The reviewer** is one person whichever surface they use: the desktop app, a browser tab, or the phone view.
- **Two kinds of agent.** An *agent harness* starts Co-Review: the agent that built the change opens a review from its own session and stays as co-reviewer. An *ACP agent* is the opposite direction: Co-Review launches it when the reviewer clicks **Connect agent** and wants an on-call expert. [How agents review with you](../agents.md) compares the two.
- **Graphify and language servers are optional.** Without Graphify the overview is built from the Tree-sitter map alone. Without a language server for a language, Co-Review's own Tree-sitter symbols still give breadcrumbs and outlines.
