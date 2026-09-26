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
3. Open it with `{{open_review}}({ dir })`. Co-Review checks it and lists problems in `format.warnings`: fix every
   one and open it again, then share the URL.
4. Answer comments in their threads and revise the design; keep the wording of commented steps stable.
5. Implement only after approval (`{{await_review}}`). If the code must depart from the approved design, update the
   design and ask again.
