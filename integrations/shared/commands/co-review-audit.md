---
description: Review the whole repository with me in Co-Review, starting with your first-pass findings grouped by area
argument-hint: "[focus, e.g. security, or a folder]"
---
Review this entire repository with me in Co-Review, following the `co-review` skill ("The whole repository"). Use the Co-Review tools from the co-review extension.

<!-- prompt: audit pi -->
1. Prepare the review without showing it: `co_review_start({ title: "Repository review", open: false })`. Keep the
   `reviewId` it returns. Don't tell me about it yet.
2. Call `co_review_map({ overview: true })` and `co_review_map`. With [Graphify](https://graphify.net) installed, the
   overview is built from its code graph (Co-Review builds it: calls, imports, clusters, the most connected code), and
   the graph's `GRAPH_REPORT.md` lists import cycles and surprising connections between distant parts of the code:
   good leads. When the repository has [Graphify](https://graphify.net) graphs, ask them how the code connects before
   searching: `graphify query "<question>"`, `graphify path "A" "B"`, `graphify explain "X"`. A large repository may
   keep one graph per folder (e.g. `services/graphify-out/`; its AGENTS.md says which), and graphs are untracked, so a
   git worktree has none: pass `--graph <path>` to the right `graph.json`, the main checkout's in a worktree (the
   first line of `git worktree list`). If the folder it names has no `graph.json` (only dated snapshots), use the
   whole repository's, `graphify-out/graph.json`. `co_review_map({ overview: true })` names the graphs it found. Only
   when there is none, say so and search instead. Split the repository (or the focus I gave you) into 4–10 areas and read the riskiest code in each
   (entry points, input handling, money, auth, concurrency, persistence).
3. Add findings with `co_review_add_findings`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do, with `severity`. A finding goes on the code (`path`,
   `line`); one about a whole area on its folder (`path` without `line`); one about the repository on `path: "."`.
   Say so in the overview when an area looks fine.
   Then write your overview of it for the review's front page, which I read first, and add it with
   `co_review_start({ reviewId, overview, open: false })`: what it does and why (a few sentences), how it works as a
   ```mermaid flowchart (the path a request or the data takes, 5–12 nodes), what else it affects (the blast radius:
   callers, data, configuration, other services), and where to start reading, as a short numbered list at the end,
   each linked to the code so I can click it (`1. [**The handler**](path/to/file.go:42): what it does`). Plain
   language; no line-by-line walkthrough.
4. Show it: `co_review_start({ reviewId })`. It opens on the overview page: your overview, every finding grouped by
   area, and the repository's areas, as one page I can read and comment on. Tell me it is open in Co-Review, with the
   areas with their finding counts, in one short message.
5. Then loop `co_review_wait` → investigate → `co_review_reply` until it returns my Submit or says I closed the review
   (closing it means I'm done: act on nothing, and tell me in one line what's still open). On my Submit: approve → nothing to change unless I say so; request changes → fix
   what I accepted and asked for, as commits, then reply in each thread with what changed and wait again. Don't
   edit files while I review unless I ask in a thread.
<!-- /prompt -->

Focus: $ARGUMENTS
