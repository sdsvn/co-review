## ADDED Requirements

### Requirement: Threads in the editor
Threads SHALL render under their code in the editor, with messages, a reply box, resolve and reopen, accept and
dismiss for proposed findings, collapse and expand all, gutter markers and hovers. Proposed findings on a pull
request SHALL stay folded to a gutter icon until opened.

#### Scenario: Reply inline
- **WHEN** the reviewer types a reply under a line and sends it
- **THEN** the message appears in the thread and reaches the agent

### Requirement: Starting a thread
The reviewer SHALL start a thread from **+** in the gutter (drag for several lines), from a selection (**Comment**
`Cmd+Alt+M`, **Ask Agent** `Cmd+Alt+A`), on a symbol, and on a file, folder or the repository from the project
panel. Asking SHALL be the default for multi-line gutter drags.

#### Scenario: Ask about a selection
- **WHEN** the reviewer selects code and presses `Cmd+Alt+A`
- **THEN** a question thread opens on that range and the connected agent is asked

### Requirement: Drafts
Unsent comments SHALL be kept per review, listed in the panel, and survive a restart.

#### Scenario: Draft survives restart
- **WHEN** the reviewer quits with an unsent comment
- **THEN** the draft is there, in place, after reopening

### Requirement: Moving between threads
`Cmd+Alt+↓` / `Cmd+Alt+↑` SHALL go to the next or previous thread, **Go to Comment…** (`Cmd+Alt+O`) SHALL list
them, and clicking a thread in the panel SHALL open it at its code.

#### Scenario: Next comment
- **WHEN** the reviewer presses `Cmd+Alt+↓`
- **THEN** the editor shows the next thread in the review

### Requirement: Thread status for agents
Each thread SHALL say where the reviewer's latest message is: waiting for the agent, or the agent is on it.

#### Scenario: Agent picks up a question
- **WHEN** the agent receives the reviewer's question through `await_reviewer`
- **THEN** the thread changes from "waiting for <agent>" to "<agent> is on it"

### Requirement: Links in messages
`path:line`, `path#L10-L20` and `/page.md` links in messages SHALL open the code or page.

#### Scenario: Link to code
- **WHEN** the reviewer clicks `src/orders.ts:42` in an answer
- **THEN** the file opens at line 42
