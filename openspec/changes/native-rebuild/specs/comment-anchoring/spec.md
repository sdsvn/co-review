## ADDED Requirements

### Requirement: Comment locations
A thread SHALL be placeable on the repository, a folder, a file, a symbol, a line, a range of lines, a rendered
document (the whole document, selected text, a Mermaid diagram, node or edge, a design step) or a patch (the
patch, a file, a line or a range, on the old or new side), using the anchor formats in `llms.txt`.

#### Scenario: Symbol comment
- **WHEN** the reviewer comments on a function's name
- **THEN** the thread is stored with the symbol's qualified path (for example `OrderService.CreateOrder`) and a
  Tree-sitter token anchor

### Requirement: Comments follow the code
Line, range and document text comments SHALL carry a tracked range (a version of the file, the commented text, 40
characters of context each side) and, whenever the review is read or a commented file changes, SHALL be resolved
against the current file to one of: `active` (unchanged, possibly moved), `modified` (changed but identified; the
original text kept), `removed`, or `ambiguous` (several places fit about equally). An edit Co-Review applies itself
(an accepted suggestion) SHALL be mapped exactly.

#### Scenario: Code moved
- **WHEN** a commented function is moved lower in the same file
- **THEN** the comment shows on the function at its new place and is `active`

#### Scenario: Code reworded
- **WHEN** the commented line is edited but still recognisable
- **THEN** the comment stays on it, marked `modified`, with the text originally commented

#### Scenario: Code deleted
- **WHEN** the commented lines are deleted
- **THEN** the comment is `removed`

#### Scenario: Never placed on untouched code
- **WHEN** the commented text was deleted and the same text exists elsewhere in code that was not edited
- **THEN** the comment is not moved onto that other code

#### Scenario: Same results as the Theia app
- **WHEN** the anchor corpus (`extensions/review/test/anchor-corpus.mjs` cases) is resolved
- **THEN** every case has the status and range the Theia app gives

### Requirement: Live placement while editing
In an open editor, comments SHALL stay on their code as the reviewer or an agent types, before the file is saved:
by symbol, by exact text, by token sequence (surviving reformatting), by text for languages without a grammar;
otherwise shown as outdated where they were, with their original code.

#### Scenario: Typing above a comment
- **WHEN** the reviewer inserts lines above a commented line without saving
- **THEN** the comment moves down with its line

### Requirement: Out of scope comments
Unresolved comments that are `removed` or `ambiguous` SHALL be out of scope: never shown on code, listed in the
panel's **Out of scope** filter, and back in scope when the code changes again so they can be placed.

#### Scenario: Back in scope
- **WHEN** a removed comment's text is restored by a later edit
- **THEN** the comment shows on the code again and leaves the Out of scope filter

#### Scenario: Agents see the status
- **WHEN** an agent reads threads (`await_reviewer`, `get_review`)
- **THEN** each line, range or text thread has `anchorStatus`, with `original` when modified and `candidates` when ambiguous
