## ADDED Requirements

### Requirement: Repository map
Co-Review SHALL provide a map of every source file with the classes, functions and methods it defines (Tree-sitter),
filterable by path and query and ordered by nearness to a focus file, as returned by `repo_map`. It SHALL be built
once per repository and reused by every worktree of it and across restarts, re-parsing only files whose content
changed.

#### Scenario: Second worktree
- **WHEN** a review opens in a new worktree of a repository already mapped
- **THEN** only files that differ from the cached content are parsed

### Requirement: Areas
Source files SHALL be grouped into areas by folder, splitting a folder that holds over 40 % of the files into its
subfolders, up to four levels.

#### Scenario: Mostly one folder
- **WHEN** most files are under `src/`
- **THEN** the areas are `src/api`, `src/db` and so on, not `src`

### Requirement: Graphify graph
When the `graphify` command is installed, Co-Review SHALL build or refresh the repository's Graphify graph (code
only, no LLM) before building the repository overview, share it across the repository's worktrees, and keep a
`graphify-out/` it created out of `git status` through `.git/info/exclude`.

#### Scenario: Restart
- **WHEN** Co-Review restarts and the repository has not changed
- **THEN** the graph is not rebuilt

### Requirement: Repository overview
`repo_map({ overview: true })` and **Review: Open Repository Overview** SHALL return where to start, the most
connected code, its clusters and a Mermaid diagram of how they connect, linking Graphify's `GRAPH_REPORT.md`; from
the repository map alone when Graphify is not installed.

#### Scenario: Without Graphify
- **WHEN** `graphify` is not installed
- **THEN** the overview is built from the repository map
