## ADDED Requirements

### Requirement: Background start
Started with `--background`, Co-Review SHALL open no window until a review is shown or the app is activated.

#### Scenario: Agent starts Co-Review
- **WHEN** an agent's first tool call starts Co-Review
- **THEN** no window appears until the agent shows a review

### Requirement: One window per repository
Showing a review, or launching Co-Review with a repository, SHALL open that repository's window or focus it if
open.

#### Scenario: Second launch
- **WHEN** `co-review /repo` runs while `/repo` has a window
- **THEN** that window is focused, and no second one opens

### Requirement: Window lifecycle
Closing a review's window or quitting SHALL tell its agents the review is closed. In an app started in the
background, a window an agent showed SHALL say why and close after 10 s once that agent is done (approved, or its
session ended), unless the reviewer clicks **Keep open**; with no window left the app SHALL quit.

#### Scenario: Approved
- **WHEN** the reviewer approves and the agent's session ends
- **THEN** the window counts down and closes unless kept open

### Requirement: Review-focused shell
Co-Review SHALL ship with Co-Review Dark and Light themes following the OS, its own name, icon and update feed, and
without debugger, tasks, REPL, collaboration, AI assistant or edit prediction in its menus and command palette.
Review commands SHALL be under `Review:`.

#### Scenario: First launch
- **WHEN** Co-Review starts for the first time
- **THEN** it shows the editor, project panel, search, Git, terminal and the Review panel, branded Co-Review only
