---
name: co-review-design
description: Design a non-trivial change before coding and have the human review it in Co-Review. Investigate the code, write the smallest design document that lets them understand, challenge and approve the approach (Context, a Design step tree of what → how → what can go wrong, optional Alternatives / Behaviour / Open Questions), open it in Co-Review, revise it from their comments, and implement only after approval. Use when the user says "design this first", "let me review the design", "write a design doc", "plan before coding", or when a change is architecturally non-trivial (new component, data model, API, async flow, migration). Needs Co-Review's MCP tools.
---

# Design first, reviewed in Co-Review

**A design document is the smallest document that lets another engineer understand, challenge and approve the
change.** It exists for review: show the decisions, assumptions, trade-offs, important behaviour, failure modes and
open questions, and nothing else. Full method and examples: Co-Review's `docs/design-docs.md`.

<!-- prompt: pi-tools -->
> **In Pi or Oh My Pi** (the Co-Review package) the tools are named `co_review_start` (open_review; `dir` for a
> review directory), `co_review_wait` (await_comment), `co_review_reply`, `co_review_add_findings`, `co_review_ask`,
> `co_review_verdict` (await_review) and `co_review_map` (repo_map). In an interactive session, the reviewer's
> questions also arrive on their own as `[Co-Review]` messages.
<!-- /prompt -->

## The steps

<!-- prompt: design -->
1. Investigate the code first and reuse existing patterns. If something new is needed (a dependency, service or
   table), make it an explicit design step. Don't invent requirements: ask under Open Questions.
2. Write `design/<name>/index.markdown`, for a person to read:

       ---
       co-review: design
       ---
       # <the change, as a short imperative>
       ## Context         why change, what happens today (a few sentences)
       ## Design          required: one nested "- " list, 2 spaces per level
                          L1 what (a behaviour, not a technology), L2 how, L3 what can go wrong
       ## Alternatives    optional: real choices, their trade-offs, why this one
       ## Behaviour       optional: runtime scenarios (failure, retry, crash, race)
       ## Open Questions  optional: decisions only the reviewer can make, as "- " items

   Plain language, one decision per step. Name code once, at the top level (entry point, module, new table), then
   describe it in words. No line numbers, no file:line links, no pasted code (Mermaid is fine). Think through
   compatibility, security, failure handling and migration, but write down only what changes the design. Size it
   to the change: about 10-30 lines if small, 30-80 medium, 80-150 large. Cut what doesn't help the reviewer decide.
3. Open it with `open_review({ dir })`. Co-Review checks it and lists problems in `format.warnings`: fix every
   one and open it again, then share the URL.
4. Answer comments in their threads and revise the design; keep the wording of commented steps stable.
5. Implement only after approval (`await_review`). If the code must depart from the approved design, update the
   design and ask again.
<!-- /prompt -->

Put the directory in a scratch location instead if the user doesn't want design documents committed.

## Answering review comments

Follow the `co-review` skill's loop (`await_comment` → investigate → `reply`). For each comment:

- Check the code if needed, then decide what the comment points at: missing information, a wrong assumption, an
  architectural problem, an open decision, or a detail that doesn't belong. Update the document and reply saying
  which step changed.
- Don't accept suggestions blindly: if one conflicts with the code, say so, with the evidence.
- **Keep commented steps' wording stable.** Comments anchor to a step's text, so rewording outdates them, and moving
  doesn't. Prefer adding or restructuring steps around a commented one. If a step is wrong, fix it anyway.
- Move settled Open Questions into the Design.

## After the verdict

`await_review` returns the decision:

- **approve**: implement the approved design, then check the code against it. If implementation shows the design is
  wrong or incomplete, stop that part, update the design, and get it approved again before continuing. Don't
  silently change the architecture. A detail that doesn't change a decision needs no new round.
- **request-changes**: revise the document, reply per thread, and wait for the next round.
- `acceptedSuggestions` on the document are already written into it by Co-Review (`doc.version` goes up). Re-read the
  file before editing.
