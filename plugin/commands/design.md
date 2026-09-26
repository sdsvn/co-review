---
description: Write a design for this task, have me review it in Co-Review, and implement only after I approve
argument-hint: "<what to design>"
---
Design this before writing code, following the `co-review-design` skill: $ARGUMENTS

1. Investigate the code first and reuse existing patterns. Don't invent requirements; ask what you can't find out.
2. Write the smallest design that lets me review the approach, in `design/<short-name>/index.markdown`
   (`co-review: design` frontmatter): Context, then a Design step tree of what (L1) → how (L2) → what can go wrong
   (L3), plus Alternatives, Behaviour or Open Questions only if they help. I read it, so write for a person: plain
   language, code named once at the top level (entry point, module, new table), no line numbers or pasted code.
   Size it to the change; self-review and cut what doesn't help me decide.
3. Open it with `open_review({ dir })`, fix every `format.warnings` item and reopen it, then give me the URL.
4. Answer my comments in their threads and revise the design (keep commented steps' wording stable). Wait for my
   verdict with `await_review`.
5. Implement only after I approve. If the implementation has to depart from the approved design, update the design
   and ask me again.
