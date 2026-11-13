# C4 diagrams

Co-Review's architecture in the [C4 model](https://c4model.com): the same system drawn at three levels of zoom, plus one flow. The diagrams are Mermaid, so they render on GitHub and in Co-Review's document view (which renders Mermaid fences with the Mermaid the app bundles), and they are meant to be read before the prose in [architecture.md](../architecture.md), which goes deeper into each part.

| Diagram | Zoom | Read it to learn |
|---|---|---|
| [System context](c4-context.md) | The whole system as one box | Who uses Co-Review and which systems it talks to |
| [Containers](c4-containers.md) | The separately running parts | How the window, the backend, the desktop shell, the CLI and agents fit together |
| [Backend components](c4-components-review-backend.md) | Inside the backend | Which service handles what, and that `ReviewStore` is the hub |
| [An agent reviews with the human](c4-dynamic-agent-review.md) | One flow, numbered | What happens between `open_review` and the verdict |

Mermaid draws C4 relation lines and labels in a fixed dark grey, so on GitHub's dark theme the arrows are faint; Co-Review's document view recolours them.

There is no deployment diagram: Co-Review runs on the reviewer's own machine, as a desktop app or a local browser app, and [Running and packaging](../running-and-packaging.md) covers both.

## Legend

Mermaid draws no legend, so the shapes mean this in every diagram:

- **Person**: a human, here always the reviewer.
- **System**: Co-Review as a whole (blue), or an external system Co-Review talks to but does not own (grey): an agent harness, an ACP agent, the git repository, GitHub, Graphify, language servers.
- **Container**: a part of Co-Review that runs as its own process or page: the review workbench, the backend, the desktop shell, the `co-review` command, `co-review-server`, the phone view. The cylinder is the review home under `~/.co-review`.
- **Component**: a service inside the backend. Components are not deployed on their own.
- **Arrows** point from the side that initiates. Each label says what happens and, after a line break, over what: JSON-RPC, MCP, ACP, the gh CLI, the file system.

## Keeping them current

The diagrams name the real classes and commands, so a rename in `extensions/review/src/node` or `bin/` should be carried here. Keep the names identical across the four files, and keep each diagram under twenty elements: split a diagram before it grows past that.
