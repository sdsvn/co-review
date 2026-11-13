This is a review of the whole repository: what is wrong with it, area by area, for someone who already knows their
way around it. If I don't yet, say so first and offer the guide (the understand command) instead: learning comes
before reviewing.

1. Before anything else, tell me in one short message that this takes a few minutes: you read the repository and
   take a first pass at its problems, and you'll tell me when it is open in Co-Review. Then prepare the review
   without showing it: `{{open_review}}({ title: "Repository audit", open: false })`. Keep the `reviewId` it returns.
2. Read before you judge: the README and docs, AGENTS.md / CLAUDE.md, the CI config, the entry points and the core
   types. Then get the facts Co-Review works out without a model: `{{repo_facts}}` for the summary, then each section
   (`entry`, `folders`, `load`, `names`, `shapes`, `history`; with a focus, pass it as `focus`). They are your
   evidence: the commit to pin the review to, the areas, the load-bearing files, the hotspots (churn × size), what
   changes together, the naming rules and where code breaks them. Use `{{repo_map}}` to find definitions.
   {{> graphify}}
3. Split the repository into its areas (4–10, from the facts' map), and look in each for what would hurt: bugs and
   unsafe assumptions, missing or misplaced checks at the trust boundaries, untested load-bearing code, code that
   breaks the house rules or the naming, duplication and dead code, and docs that no longer match the code. Read the
   code before you claim anything about it.
4. Write a short overview, the review's front page, with `{{open_review}}({ reviewId, overview, open: false })`:
   - First line: "Reviewed at `<short sha>` on <date>.", the facts' commit. Then what the repository does in two
     or three sentences, and a C4 context diagram (```mermaid `C4Context`: the system and what it talks to).
   - The areas: one row each, with what it is and the one thing most worth my attention there.
   - Where it hurts: the load-bearing code (what most depends on), the hotspots, what changes together, what is
     untested or dead, and the trust boundaries. Say in one line when there is too little history to tell.
   Link every claim to its code ([`UserStore.create`](path/to/file.ts:42)), tag what you didn't verify _(inferred
   from …)_, and keep Mermaid plain: in a C4 diagram, every element a description, every arrow a verb of at most five
   words and its protocol, under 15 elements and 10 arrows, ending with `UpdateLayoutConfig($c4ShapeInRow="3")`.
5. Add your findings with `{{add_findings}}`: the area as the first label, `status: "proposed"`, at most three per
   area, each with what's wrong, why it matters and what to do, with `severity`, on the code (`path` and `code`, the
   line(s) copied exactly from the file), a folder (`path` alone) or the repository (`path: "."`).
6. Show it: `{{open_review}}({ reviewId })`. It opens on the overview page: your overview, then your findings grouped
   by area, then the repository's areas. Tell me it is open in Co-Review, with how many findings in which areas, in
   one short message.
7. Then loop `{{await_reviewer}}` → investigate → `{{reply}}` until it returns my Submit or says I closed the review
   (closing it means I'm done: act on nothing, and tell me in one line what's still open). On my Submit: approve →
   nothing to change unless I say so; request changes → fix what I accepted and asked for, as commits, then reply in
   each thread with what changed and wait again. Don't edit files while I read unless I ask in a thread.
