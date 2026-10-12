## ADDED Requirements

### Requirement: Rendered Markdown pages
Markdown documents SHALL open rendered, in review directories and in code reviews, with comments on selected text
(**Comment**, **Ask Agent**) and on the whole document. Opening at a line SHALL go to the editor. HTML SHALL NOT be
rendered; **Open in Browser** hands it to the system browser.

#### Scenario: Text comment
- **WHEN** the reviewer selects a sentence and comments
- **THEN** the thread has a `text` anchor with the exact text, prefix and suffix

### Requirement: Mermaid comments
Mermaid blocks SHALL render as diagrams with comments on a diagram, a node or an edge (`%% id:` block ids, explicit
node ids).

#### Scenario: Node comment
- **WHEN** the reviewer comments on a node
- **THEN** the thread is anchored with `mermaid-node`, its `blockId` and `nodeId`

### Requirement: Design tree
A `## Design` section (or a `*.pseudocode.md` file) SHALL render as an L1 · L2 · L3 tree with level switching,
`?` `⚠` `✎` badges, the reason after ` — ` muted, and a comment action per step.

#### Scenario: Step comment
- **WHEN** the reviewer comments on a step
- **THEN** the thread is anchored with `tree-node` to the step's text

### Requirement: Design format check
A document declared `co-review: design` SHALL be checked against the format in `docs/design-docs.md`, and its
departures returned in `format.warnings` (and `doc.format`) and shown as a banner.

#### Scenario: Line reference in a design
- **WHEN** a design step contains a `file:line` reference
- **THEN** `open_review` returns a warning for it

### Requirement: Suggested document edits
The reviewer and agents SHALL be able to suggest edits to document text; an accepted edit SHALL be written into the
document and increment `doc.version`.

#### Scenario: Accept an edit
- **WHEN** the reviewer accepts a suggested rewrite
- **THEN** the document file contains the new text

### Requirement: Review directories
`open_review({ dir })` SHALL find the document (`index.markdown`, `index.md`, `INDEX.md`, `README.md`,
`readme.md`, then `*.pseudocode.md`), open each `*.patch`/`*.diff`, read `PR.md` / `<patch>.md`, ingest
`<patch>.comments.json(l)` as proposed findings idempotently and live, accept inline `markdown`/`patch`, and write
the state to `review.json` (or `storePath`).

#### Scenario: Sidecar findings updated
- **WHEN** a findings file gains an entry while the review is open
- **THEN** the new finding appears, and existing ones are not duplicated

### Requirement: Knowledge bundles and OpenSpec
Every Markdown page of a review directory (such as an OKF bundle) SHALL be a review page (`doc:<path>`) with its
frontmatter (`type`, `description`, `resource`) as a header and links resolving to the code. An `openspec/` change
SHALL render as cards under the document.

#### Scenario: Code link in a bundle page
- **WHEN** the reviewer clicks `src/limiter.go:42` on a bundle page
- **THEN** the repository file opens at that line
