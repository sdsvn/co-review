## ADDED Requirements

### Requirement: MCP endpoint
Co-Review SHALL serve MCP over Streamable HTTP at `http://127.0.0.1:<port>/mcp?root=<repo>` to local connections
only, record the running instance in `~/.co-review/server.json`, and give the server instructions of the Theia app.

#### Scenario: Remote connection refused
- **WHEN** a client on another host connects to the MCP endpoint
- **THEN** the connection is refused

### Requirement: Tool contract
The tools `open_review`, `await_reviewer`, `reply`, `add_findings`, `ask_reviewer`, `post_review_to_github`,
`repo_map` and `get_review` SHALL take the arguments and return the results documented in `llms.txt`, with one
exception: `open_review` returns a `note`, never a `url`.

#### Scenario: Recorded sessions replay
- **WHEN** MCP sessions recorded from the Theia app for the four workflows are replayed
- **THEN** every result matches, ids, times and the `url`/`note` difference aside

### Requirement: Delivery
`await_reviewer` SHALL return the reviewer's next move: comments needing the agent (each delivered once per MCP
session), the submission (after comments still waiting), `closed` when the reviewer closed the window or quit (also
after a restart), or `pending` on timeout. `ask_reviewer` SHALL show its options as buttons in a thread and wait
the same way.

#### Scenario: Window closed
- **WHEN** the reviewer closes the review's window while an agent waits
- **THEN** `await_reviewer` returns `closed`

### Requirement: Presence
The agent SHALL be shown as listening while it waits in `await_reviewer` or `ask_reviewer`.

#### Scenario: Agent busy
- **WHEN** the agent is not waiting
- **THEN** the panel shows it as not listening, and questions wait for its next call

### Requirement: Claude Code channel
For Claude Code clients, Co-Review SHALL push the reviewer's comments and submissions into the session as channel
messages with `thread_id` and `event`.

#### Scenario: Live question
- **WHEN** the reviewer asks a question while the channel is on
- **THEN** the Claude Code session receives it at once
