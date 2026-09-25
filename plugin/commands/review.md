---
description: Open this change in Co-Review and stay as my co-reviewer until I submit
argument-hint: "[what to focus on]"
---
Open a Co-Review review of this repository and be my co-reviewer, following the `co-review` skill:

1. Call `open_review` for this repository and give me the URL.
2. Add findings (`add_findings`) only for real risks or non-obvious decisions in what you changed in this session:
   at most five, with file and line.
3. Then loop `await_comment` → investigate → `reply` until I submit the review, and act on my verdict
   (`await_review`). Don't edit files while I review unless I ask in a thread.

Focus: ${ARGUMENTS:-the changes you made in this session}
