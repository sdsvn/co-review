## ADDED Requirements

### Requirement: Review panel
The panel SHALL show the review switcher, the review's scope, its agent (model, listening status on hover), drafts,
threads grouped by file or **By area** (first label), filters **Open**, **Proposed**, **Resolved**, **Out of
scope** and **All**, an **Overview** link and a **⋯** menu of review actions.

#### Scenario: Group by area
- **WHEN** threads have labels and the reviewer clicks **By area**
- **THEN** threads are grouped by their first label

### Requirement: Submit review
**Submit review…** SHALL send one verdict (Approve, Request changes or Comment) with a message, and deliver it with
every open comment and accepted suggestion to the agent. The submit bar SHALL say whether the agent has it yet.

#### Scenario: Submit with no agent connected
- **WHEN** the reviewer submits while no agent is connected
- **THEN** the next agent that joins the review receives the submission

### Requirement: Viewed files and coverage
The reviewer SHALL mark files and folders as viewed (`Cmd+Alt+V`); a file changed since counts as not viewed.
Coverage SHALL show viewed files overall and per area, with each area's next unviewed file.

#### Scenario: Changed file loses viewed
- **WHEN** a file marked viewed is modified
- **THEN** it counts as not viewed in coverage
