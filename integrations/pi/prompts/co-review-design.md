---
description: Write a design for this task, have me review it in Co-Review, and implement only after I approve
argument-hint: "<what to design>"
---
Design this before writing code, following the `co-review-design` skill: $ARGUMENTS

Use the Co-Review tools from the co-review extension (`co_review_start` opens the review; for a design directory,
pass its path as `dir`).

1. Write `design/<short-name>/index.markdown` with `co-review: design` frontmatter: Context, Design (one nested
   `- ` step tree, L1 business language → L2 mechanism → L3 edge cases), Diagrams (Mermaid with `%% id:`), Open
   Questions.
2. Open it with `open_review({ dir })`, fix any `format.warnings`, and give me the URL.
3. Answer my comments in their threads, revise the document when a comment changes the design (keep step wording
   stable), and wait for my verdict with `await_review`.
4. Implement only after I approve.
