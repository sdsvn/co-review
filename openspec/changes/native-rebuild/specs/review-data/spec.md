## ADDED Requirements

### Requirement: Review model
A review SHALL hold a title, its repository (or review directory), a scope, participants, numbered threads, an
activity log, the reviewer's drafts and, once connected, its agent. A thread SHALL hold a location, messages
(author, role human or agent, time, Markdown body), a status (`open`, `proposed`, `resolved`), an intent
(`comment` or `question`), and optionally a severity, labels and a suggested edit. Threads SHALL be numbered in
order of creation and addressable by short id `t<N>` or full id.

#### Scenario: Short thread ids
- **WHEN** an agent replies to thread `t5`
- **THEN** the reply lands in the review's fifth thread, exactly as if its full id had been given

#### Scenario: Proposed finding
- **WHEN** an agent adds a finding with status `proposed`
- **THEN** it is shown as a finding to accept or dismiss, not as an open comment

### Requirement: Storage outside the repository
Reviews SHALL be stored as JSON under `~/.co-review/workspaces/<sha256(repository URI)[0:16]>/reviews/<id>.json`,
with `workspace.json` recording the repository path, and `CO_REVIEW_HOME` SHALL replace `~/.co-review`. The
repository itself SHALL never be written to for review state.

#### Scenario: Custom home
- **WHEN** Co-Review runs with `CO_REVIEW_HOME` set to a folder
- **THEN** every review, snapshot, page, worktree, log and `server.json` is under that folder

#### Scenario: Windows paths
- **WHEN** the repository is `C:\src\app` on Windows
- **THEN** its reviews are found under the same hashed folder whichever app version wrote them

### Requirement: Data compatible with the Theia app
Co-Review SHALL read reviews written by the Theia app (release 0.6.x) without migration, and SHALL write reviews
the Theia app can read.

#### Scenario: Existing reviews open
- **WHEN** the new app first starts on a `~/.co-review` the Theia app wrote
- **THEN** every review appears with its threads, messages, drafts, statuses and viewed files

### Requirement: Consistent state for every participant
Every change to a review (from the reviewer, an MCP agent, an ACP agent or a sidecar file) SHALL be
applied one at a time per review, saved, and shown to every open window and connected agent.

#### Scenario: Simultaneous changes
- **WHEN** the reviewer replies in a thread at the same moment an agent adds a finding
- **THEN** both are saved and both appear for everyone

### Requirement: Review kinds
Co-Review SHALL support four kinds of review: a whole repository, a change (a diff of the repository), a pull
request (a review directory with a patch and `PR.md`), and a document review (a review directory with a design
document or knowledge bundle).

#### Scenario: Each kind opens
- **WHEN** an agent calls `open_review` with `root`, with `diff`, with `dir` and `diff`, or with `dir` alone
- **THEN** a repository, change, pull-request or document review is created respectively

### Requirement: Review lifecycle
The reviewer SHALL be able to create a review by hand, rename, delete, archive and unarchive a review, list
archived reviews, and switch between several reviews of the same repository.

#### Scenario: Archive and restore
- **WHEN** the reviewer archives a review and later unarchives it
- **THEN** it leaves the review switcher, appears under archived reviews, and comes back with its threads intact
