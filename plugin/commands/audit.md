---
description: Review the whole repository with me in Co-Review, starting with your first-pass findings grouped by area
argument-hint: "[focus, e.g. security, or a folder]"
---
Review this entire repository with me in Co-Review, following the `co-review` skill ("Reviewing the whole
repository"):

<!-- prompt: audit -->
1. Call `open_review` for this repository with a `title` such as "Repository review", and give me the URL.
2. Call `repo_map`, split the repository into 4–10 areas, and read the riskiest code in each.
3. Add findings with `add_findings`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do.
4. Tell me the areas and how many findings each has, in one short message.
5. Then loop `await_comment` → investigate → `reply` until I submit, and act on my verdict (`await_review`).
   Don't edit files while I review unless I ask in a thread.
<!-- /prompt -->

Focus: ${ARGUMENTS:-the whole repository}
