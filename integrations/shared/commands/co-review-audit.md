---
description: Review the whole repository with me in Co-Review, starting with your first-pass findings grouped by area
argument-hint: "[focus, e.g. security, or a folder]"
---
Review this entire repository with me in Co-Review, following the `co-review` skill ("The whole repository"). Use the Co-Review tools from the co-review extension.

<!-- prompt: audit pi -->
1. Prepare the review without showing it: `co_review_start({ title: "Repository review", open: false })`. Keep the
   `reviewId` it returns. Don't give me a URL yet.
2. Call `co_review_map({ overview: true })` and `co_review_map`. With [Graphify](https://graphify.net) installed, the
   overview is built from its code graph (Co-Review builds it: calls, imports, clusters, the most connected code), and
   `graphify-out/GRAPH_REPORT.md` lists import cycles and surprising connections between distant parts of the code:
   good leads. Split the repository into 4–10 areas and read the riskiest code in each (entry points, input handling,
   money, auth, concurrency, persistence).
3. Add findings with `co_review_add_findings`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do. Say so when an area looks fine.
4. Show it: `co_review_start({ reviewId })`. It opens on the findings page: every finding, grouped by area, as one
   page I can read and comment on. Give me the URL (if there is one) and the areas with their finding counts, in one
   short message.
5. Then loop `co_review_wait` → investigate → `co_review_reply` until it returns my Submit, and act on my verdict.
   Don't edit files while I review unless I ask in a thread.
<!-- /prompt -->

Focus: $ARGUMENTS
