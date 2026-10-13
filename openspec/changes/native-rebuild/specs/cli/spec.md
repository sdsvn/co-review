## ADDED Requirements

### Requirement: The co-review command
`co-review [dir]` SHALL open or focus `dir`'s window; `co-review status [dir]` SHALL print the app's and each
integration's version (with the update command for any that differ) and the open reviews; `co-review setup pi|omp`
SHALL install the bundled package; `co-review logs` SHALL say whether Co-Review answers and print the recent logs of
`~/.co-review/logs` merged by time.

#### Scenario: Version mismatch
- **WHEN** the Claude Code plugin is older than the app
- **THEN** `co-review status` flags it with the command that updates it

### Requirement: Stdio MCP bridge
`co-review mcp [dir]` SHALL answer the agent's handshake itself, start Co-Review in the background on the first
tool call, fail waiting calls if Co-Review goes away and reconnect on the next, and end its session when the
harness ends.

#### Scenario: Session start
- **WHEN** an agent session starts with the bridge configured
- **THEN** Co-Review is not started until the first tool call

### Requirement: Stale versions
A running Co-Review SHALL quit when the version on disk is not its own, and the command SHALL stop a running app of
another version, or one without a working backend, before starting.

#### Scenario: After an update
- **WHEN** a new version is installed while the old one runs
- **THEN** the next `co-review` call runs the new version

### Requirement: In-app setup
**Install Agent Skills…**, **Install the co-review Command** and **Add Co-Review to an Agent Harness (MCP)…** SHALL
install the bundled skills, the command and each harness's MCP configuration.

#### Scenario: Add to a harness
- **WHEN** the reviewer adds Co-Review to Codex
- **THEN** Codex's MCP configuration runs `co-review mcp` with this machine's paths
