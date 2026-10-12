## ADDED Requirements

### Requirement: Connect an agent
**Connect agent…** SHALL offer the installed ACP agents (Claude Code, Gemini CLI, OpenCode, Goose, Oh My Pi, Pi,
Codex) or any ACP command, then the agent's own model and reasoning-effort options, stored with the review and
applied to every session.

#### Scenario: Pick a model
- **WHEN** the reviewer connects Claude Code and picks a model
- **THEN** every question in the review is answered with that model

### Requirement: One session per thread
Each thread SHALL have its own ACP session in the repository root, started ahead of time. The first prompt SHALL
carry the review and thread context, the code, a link to the file, the answer style and the nearest part of the
repository map if ready within 2 s; later prompts only the new messages.

#### Scenario: Follow-up
- **WHEN** the reviewer replies in a thread the agent answered
- **THEN** the same session continues

### Requirement: Answers in the thread
The answer SHALL stream into the thread, earlier messages in the turn shown as steps and tool calls as activity
lines; permission requests SHALL be buttons; **Stop** SHALL cancel the turn. File access SHALL be limited to the
repository, with no terminal.

#### Scenario: Permission request
- **WHEN** the agent asks for permission
- **THEN** its options appear as buttons and the turn waits for the reviewer
