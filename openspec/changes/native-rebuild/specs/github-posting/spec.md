## ADDED Requirements

### Requirement: Post a review to its pull request
For a review whose `PR.md` names a pull request, Co-Review SHALL post one GitHub review through `gh`: the verdict as
the event, the message as the summary, open diff-line threads the reviewer wrote or accepted as line comments (the
reviewer's point only), accepted suggestions as GitHub suggestions, other comments in the summary. Agent replies,
questions to an agent, unaccepted findings, resolved threads and the agent's own threads SHALL NOT be posted.

#### Scenario: Own pull request
- **WHEN** the reviewer approves their own pull request
- **THEN** it is posted as a comment, and the reviewer is told why

#### Scenario: Line left the diff
- **WHEN** a commented line is no longer in the diff
- **THEN** its comment goes into the summary instead of failing the post

### Requirement: Preview before posting
Before posting, Co-Review SHALL show exactly what will be posted, every part editable, and post only on **Post to
GitHub**. Posting SHALL be offered in Submit review, in **⋯ → Post review to GitHub…**, and to agents through
`post_review_to_github`, which shows the preview in a thread.

#### Scenario: Edited line comment
- **WHEN** the reviewer empties a line comment in the preview and posts
- **THEN** that comment is left out
