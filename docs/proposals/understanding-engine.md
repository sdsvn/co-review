---
co-review: design
---
# Make the repository guide a fixed structure that Co-Review fills, checks and draws

Status: approved 2026-10-30. Step 1, the facts (`repo_facts`), is implemented; steps 2–4 are not. Background: [the
proposal](understanding-a-repository.md), [ADR 0001](../adr/0001-understanding-first-repository-review.md) and
[the method as pseudocode](understanding-a-repository.pseudocode.md).

## Context

`/co-review:audit` is one long prompt today. The agent gathers its own evidence, writes the guide as free
Markdown in the review's overview, and is only asked to link claims, cover every entry point and tag guesses. Nothing
checks that it did, so the guide is only as good as the agent: a weaker one skips the history, guesses folder roles,
or leaves flows out, and the page looks just as confident. This design makes Co-Review own the structure: it computes
the facts that need no model, defines one opinionated guide shape, refuses a guide that doesn't fit it, and draws the
page itself. The agent is left with what only a model can do: explain. It follows the method in
`docs/proposals/understanding-a-repository.pseudocode.md`; its ⚙ steps move into Co-Review, its 🧠 steps stay with
the agent.

## Design

- Co-Review computes the repository's facts itself: a new `repo_facts` MCP tool, backed by a new `repo-facts.ts`
  - It builds on what exists: the Tree-sitter map in `RepoIndex`, the Graphify graph from `graphify.ts`, and plain
    git through the same `execFile` helper the code folders use.
  - It returns versioned JSON pinned to the commit, one section at a time so each answer stays small.
    - Sections: entry points, folders, load-bearing, names, shapes, history.
  - It is cached per commit next to the repository map, outside the repository.
    - The same commit always gives the same facts: that is what a later Rust port is tested against.
    - A dirty working tree is reported as such; the facts describe the files on disk, the sha says where they started.
- It finds every entry point and external contract, so flows aren't picked by taste
  - From manifests (package bins and scripts, `cmd/` folders, Cargo bins, Makefile targets) and main-like files.
  - From registration calls repeated across files (tools, routes, commands), found with Tree-sitter.
    - A registration style it doesn't know is missed. The agent may add entry points with evidence, and the page
      says which came from the agent.
- It classifies every folder and decides how deep the map goes
  - Generated (a do-not-edit header, a `.gen.` name, codegen output), vendored, tests and tooling by name and content.
  - The rest by graph position: imported by many is infra, importing many is glue, owning shared types is core.
  - The map descends while a folder mixes roles or holds over ~40% of the files, at most four levels.
  - The repository map gains each file's imports (a Tree-sitter query per language, bumping the map's version), so
    there is a dependency graph even without Graphify.
    - Imports are coarser than Graphify's calls. A role from imports alone is marked low confidence.
- It ranks the load-bearing code by importers and callers (in-degree, then PageRank).
- It mines naming and repeated shapes
  - Identifiers are split into words and counted per kind (folder, file, type, function, test).
    - A prefix, suffix, verb or casing covering ~70% of a kind, with at least five instances, is a rule.
  - Repeated shapes: sibling-file sets that recur across folders, and registration calls repeated three times or more.
    - Comparing function bodies by syntax skeleton is left for later.
- It mines the history with git
  - Churn per file, pairs that change together across areas, and authors per area.
  - Candidate recipes: commits titled add, new or introduce whose set of touched files recurs.
  - Only the last 2,000 commits; commits touching over ~50 files are ignored; under ~50 commits is thin history.
- The guide is a fixed, opinionated structure, not Markdown: `open_review({ reviewId, guide })`
  - Four chapters with set sections, every one required:
    - lay of the land: purpose, boundary, context, commands, map
    - how it works: glossary, cast of characters, entities, flows
    - patterns, naming and why: patterns, naming, where to look, house rules, recipes, decisions
    - where it hurts: load-bearing, hotspots, coupling, gaps, trust boundaries
  - Each section is a list of typed items with set fields, and every item holds claims.
    - A folder note: path, job, role, key files. A flow: entry point, sequence diagram, numbered stops, failures.
    - A house rule: the rule, an example, how it is enforced, what breaks without it.
  - A claim is one sentence, a tag (verified, inferred from what, unknown) and references to a symbol or quoted line.
    - A structural claim ("A calls B") also names the graph edge it relies on.
  - Each chapter has exactly one check: the question, and its answer key, which is stored but never drawn.
- `open_review` refuses any guide that breaks a rule, and checks each chapter as soon as it is sent
  - The agent sends the guide one chapter at a time, so a problem is caught before the next chapter builds on it.
    Nothing is shown to the reader until all four chapters are accepted.
  - A wrong shape, a missing section, or a chapter without a check is refused, with every problem listed.
  - The guide must cover the facts, or it is refused too:
    - every entry point has a flow, or is listed as not traced with a reason
    - every mapped folder has a note; every naming rule and repeated shape is explained or dismissed
  - References are resolved: a symbol through the repository map, a quote through the existing code-quote matcher.
    - A claim tagged verified whose reference doesn't resolve, or whose edge isn't in the graph, is refused too.
    - The agent fixes the reference or retags the claim as inferred itself, and sends the chapter again.
  - A section may say "too little history" only when the facts say history is thin.
    - Then decisions and recipes may be one paragraph each; nothing else may shrink.
- Co-Review stores the guide on the review and draws it
  - A new optional `guide` on the review model, next to `overview`, which keeps working for everything else.
  - Every accepted guide is kept, one per commit, outside the repository; the latest is drawn, and the older ones
    are what "since you last read this" will be computed against.
  - The overview page draws the four chapters in a fixed order and layout, then the findings as today.
    - The map is a table of folder notes and roles; flows show their diagram and stops; references link to lines.
    - Inferred and unknown claims are visibly marked; each check is its own commentable block.
  - Each claim has a stable id, so a comment reaches the agent as that claim and one claim can be corrected alone.
- A verifier checks a sample of verified claims before the reader sees them
  - Co-Review picks every decision plus a random fifth of the other verified claims.
  - An agent that sees only the code and the claim says whether it is true.
  - A claim the verifier rejects is downgraded to inferred with the verifier's reason, and the writing agent is told.
    - The verifier is Co-Review's own ACP agent when one is configured, else a subagent the writer starts; the page
      says which, since a subagent is less independent.
- The evaluation harness becomes a repeatable check, kept in the repository as a developer script
  - `bin/eval-understanding.mjs`: one agent writes held-out questions, one answers from the guide, one grades.
  - Pass: no confident-but-wrong answers, three-quarters correct, no wrong claims sampled.
    - On demand, not in CI: it costs model calls.
- The prompt shrinks to the model's part
  - `prompts/audit.md` becomes: read the docs, get the facts, fill the slots, fix every warning, converse.
  - The pseudocode document gains the guide's schema as its pieces, so method and structure stay in sync.
- It ships in four steps, each usable alone
  - The facts tool first, with today's prompt told to use it; then the checked structure and drawing.
  - Then the verifier, then the evaluation script.

## Alternatives

- **Keep Markdown and lint it** (required headings, link counts): cheaper, but it can't tell a claim from prose or
  check coverage against the facts, and agents learn to satisfy a linter's surface. A typed structure makes
  "complete" and "true" checkable.
- **Co-Review writes the whole guide itself through ACP**: removes the dependency on the calling agent, but takes
  the conversation away from the agent the user is working with, and needs a configured ACP agent. Kept for the
  verifier only.
- **Graphify required**: better edges, but it's optional today and installing it shouldn't be the price of a guide.
  Imports from Tree-sitter are the floor.

## Behaviour

- A chapter keeps failing: each refusal lists every remaining problem. After three refusals of one chapter the agent
  stops and tells the user what it couldn't satisfy; nothing half-checked is shown.
- The repository changes while the guide is being written: the guide carries the sha it was written for, and
  validation resolves references against the files on disk. A reference that moved is found again by symbol.
- A huge repository: the facts respect the map's existing 5,000-file cap and say when they were truncated. The guide
  may then cover a focus folder, as `/co-review:audit <focus>` does today.
