## ADDED Requirements

### Requirement: Overview page
Every review except a document review SHALL open on its overview page: the pull request from `PR.md`, the agent's
overview (`open_review`'s `overview`), the files changed, every thread grouped by area (severity, status, link,
code, conversation), and for a repository review the repository's areas. It SHALL be commentable like any rendered
page, its comments reaching agents as `target: "overview"` with the quoted text, and reopenable with **Overview**
and **Review: Open Review Overview**.

#### Scenario: Comment on the overview
- **WHEN** the reviewer selects text on the overview and asks the agent
- **THEN** the agent receives the question with `target: "overview"` and the selected text

### Requirement: Never waits on indexing
The page SHALL be written at once with what is cheap, say that the repository overview or blast radius is being
built, and update itself when it is ready and whenever the review changes.

#### Scenario: Large repository
- **WHEN** a review of a large repository is shown
- **THEN** the window and page appear before the index finishes, and the repository section fills in later
