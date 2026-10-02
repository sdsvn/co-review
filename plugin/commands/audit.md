---
description: Review the whole repository with me in Co-Review, starting with your first-pass findings grouped by area
argument-hint: "[focus, e.g. security, or a folder]"
---
Review this entire repository with me in Co-Review, following the `co-review` skill ("The whole repository"):

<!-- prompt: audit -->
1. Prepare the review without showing it: `open_review({ title: "Repository review", open: false })`. Keep the
   `reviewId` it returns. Don't give me a URL yet.
2. Call `repo_map({ overview: true })` and `repo_map`. With [Graphify](https://graphify.net) installed, the
   overview is built from its code graph (Co-Review builds it: calls, imports, clusters, the most connected code), and
   `graphify-out/GRAPH_REPORT.md` lists import cycles and surprising connections between distant parts of the code:
   good leads. Split the repository (or the focus I gave you) into 4–10 areas and read the riskiest code in each
   (entry points, input handling, money, auth, concurrency, persistence).
3. Add findings with `add_findings`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do, with `severity`. A finding goes on the code (`path`,
   `line`); one about a whole area on its folder (`path` without `line`); one about the repository on `path: "."`.
   Say so in the overview when an area looks fine.
   Then write your overview of it for the review's front page, which I read first, and add it with
   `open_review({ reviewId, overview, open: false })`: what it does and why (a few sentences), how it works as a
   ```mermaid flowchart (the path a request or the data takes, 5–12 nodes), what else it affects (the blast radius:
   callers, data, configuration, other services), and where to start reading. Plain language; no line-by-line
   walkthrough.
4. Show it: `open_review({ reviewId })`. It opens on the overview page: your overview, every finding grouped by
   area, and the repository's areas, as one page I can read and comment on. Give me the URL (if there is one) and the
   areas with their finding counts, in one short message.
5. Then loop `await_reviewer` → investigate → `reply` until it returns my Submit or says I closed the review
   (tell me what's still open). On my Submit: approve → nothing to change unless I say so; request changes → fix
   what I accepted and asked for, as commits, then reply in each thread with what changed and wait again. Don't
   edit files while I review unless I ask in a thread.
<!-- /prompt -->

Focus: ${ARGUMENTS:-the whole repository}
