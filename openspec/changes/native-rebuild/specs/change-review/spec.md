## ADDED Requirements

### Requirement: Diffs taken by Co-Review
`open_review` with `diff` SHALL run the git diff itself (`HEAD` includes untracked files; `base...HEAD` for a
branch) and open it as a patch page; patches in a review directory (`git diff`, `--cached`, `format-patch`) SHALL
open the same way.

#### Scenario: Uncommitted work
- **WHEN** an agent calls `open_review({ diff: "HEAD" })`
- **THEN** the patch page includes new untracked files

### Requirement: Pull-request page
A patch SHALL render as a pull-request page: the `PR.md` header, files changed with lines added and removed, and
hunks, with comments on the patch, a file, a line or a range, on either side, and suggested edits.

#### Scenario: Old-side comment
- **WHEN** the reviewer comments on a removed line
- **THEN** the thread is anchored to that line on the old side

### Requirement: From the diff into the code
A new-side line number SHALL open the head file there; an old-side one the base version, read-only. Go to
definition and hover SHALL work on the page through the language server. **Compare** SHALL open base and head side
by side.

#### Scenario: Definition from the diff
- **WHEN** the reviewer Cmd-clicks a name on the patch page
- **THEN** its definition opens

### Requirement: Code folders
A change review SHALL open on its code: the repository itself for the reviewer's own change, or a git worktree of a
pull request's head under `~/.co-review/worktrees/`, its changed neighbourhood checked out first and the rest in the
background. The worktree SHALL be removed on archive or delete and recreated on unarchive.

#### Scenario: Pull request window
- **WHEN** a pull-request review is shown
- **THEN** the window opens on the worktree before the full checkout finishes

### Requirement: The change in the project panel
Changed files and their folders SHALL be coloured with lines added and removed. **Changed files only** SHALL narrow
the panel to them (on by default for a pull request, remembered per review). A changed file SHALL open as base and
head; a new file with its added lines marked.

#### Scenario: Show all files
- **WHEN** the reviewer clicks the **Changed files only** label
- **THEN** all files are listed, and the choice is kept for this review

### Requirement: Diff comments from the editor
A comment written in the editor on a line of the diff SHALL be a comment on the diff, and diff threads on new-side
lines SHALL also show in the head file's editor.

#### Scenario: Comment in the head file
- **WHEN** the reviewer comments in the editor on an added line
- **THEN** the thread is on the patch page and would be posted to GitHub

### Requirement: Blast radius
When the repository has a Graphify graph, a change review SHALL show the definitions the change touches, what
depends on them (two hops), what they call, and a chart.

#### Scenario: Caller of a changed function
- **WHEN** a change edits a function another file calls
- **THEN** the caller is listed as affected

### Requirement: Accepted suggestions
Suggested edits the reviewer accepts on a patch SHALL be returned to the agent as `acceptedSuggestions` to commit.

#### Scenario: Accept a suggestion
- **WHEN** the reviewer accepts a suggested edit and submits
- **THEN** the submission lists it with path, line, before and after
