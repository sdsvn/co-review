---
description: Review the whole repository with me in Co-Review, starting with your first-pass findings grouped by area
argument-hint: "[focus, e.g. security, or a folder]"
---
Review this entire repository with me in Co-Review, following the `co-review` skill ("Reviewing the whole
repository"). Use the Co-Review tools from the co-review extension.

1. Call `co_review_start` with a `title` such as "Repository review" and give me the URL.
2. Call `co_review_map`, split the repository into 4–10 areas, and read the riskiest code in each.
3. Add findings with `co_review_add_findings`: the area as the first label, `status: "proposed"`, at most three
   per area, each with what's wrong, why it matters and what to do.
4. Tell me the areas and how many findings each has, in one short message.
5. Then keep answering with `co_review_wait` → investigate → `co_review_reply` until I submit
   (`co_review_verdict`). Don't edit files while I review unless I ask.

Focus: $ARGUMENTS
