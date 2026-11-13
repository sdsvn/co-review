---
description: Understand the whole repository with me in Co-Review: a guide to what it does, its map, vocabulary, flows, patterns, naming and how to change it, with C4 diagrams, then your answers to my questions. No review, no verdict
argument-hint: "[focus, e.g. a folder or a feature]"
---
Help me understand this entire repository in Co-Review, following the `co-review` skill ("Understanding the whole repository"). Use the Co-Review tools from the co-review extension.

<!-- prompt: understand pi -->
This is for learning the repository, not reviewing it. The goal is that afterwards I can, without you: say what the
repository does and where everything lives, use its vocabulary, trace its main flows, recognise its patterns and
naming, say why it is built this way, follow its house rules, and place a change and predict what it affects.
Familiarity is not the goal; being able to predict is. Explain what is there and why; don't judge it.

1. Before anything else, tell me in one short message that this takes a few minutes: you read the repository and
   write a guide to it, and you'll tell me when it is open in Co-Review. Then prepare it without showing it, as an
   explanation (no verdict, I close it when I'm done): `co_review_start({ title: "Repository guide", explain: true,
   open: false })`. Keep the `reviewId` it returns.
2. Read before you write: the README and docs, AGENTS.md / CLAUDE.md, the CI config, the entry points and the core
   types. Then get the facts Co-Review works out without a model: `co_review_facts` for the summary, then each section
   (`entry`, `folders`, `load`, `names`, `shapes`, `history`; with a focus, pass it as `focus`). They are your
   evidence: the commit to pin the guide to, the entry points, every folder's role and why, the load-bearing files,
   the naming rules, the repeated shapes, what changes together, and the recipes from commits that created a file.
   Use `co_review_map` to find definitions. When the repository has [Graphify](https://graphify.net) graphs, ask them how the code connects before
   searching: `graphify query "<question>"`, `graphify path "A" "B"`, `graphify explain "X"`. A large repository may
   keep one graph per folder (e.g. `services/graphify-out/`; its AGENTS.md says which), and graphs are untracked, so a
   git worktree has none: pass `--graph <path>` to the right `graph.json`, the main checkout's in a worktree (the
   first line of `git worktree list`). If the folder it names has no `graph.json` (only dated snapshots), use the
   whole repository's, `graphify-out/graph.json`. `co_review_map({ overview: true })` names the graphs it found. Only
   when there is none, say so and search instead.
3. Everything the facts list goes in the guide; don't pick by taste. Each entry point gets a flow in chapter 2 or is
   named there as not traced (registrations of one kind, like the routes of one API, may share a flow). Each mapped
   folder gets a row in the map. Each naming rule, repeated shape and recipe is explained in chapter 3 or 4, or
   dismissed with a reason. A role marked `low` is a guess from imports: confirm or correct it from the code, and say
   which. When `history.thin` is true, say so instead of inferring from history.
4. Write the guide as the review's overview, four chapters (`##` headings), in this order:
   - **1. Lay of the land.** First line: "Written for `<short sha>` on <date>.", the facts' commit. Then what it does,
     for whom, and what it doesn't do; a C4 context diagram (```mermaid `C4Context`: who uses the system and what it
     talks to, from the outbound calls and environment in the facts), and, when the repository builds more than one
     running part (a server and a CLI, a frontend and a backend, workers, a database), a C4 container diagram
     (`C4Container`: the separately running parts and stores, from the entry points and the folder roles; a library
     is not a container); how to build, run, test and deploy it (the actual commands). Then the map: a row per
     folder in the facts' map, with its one-line job, its role (core, infra, glue, tooling, tests, generated,
     vendored) and its key files; the facts' ignore list in one "you can ignore" line.
   - **2. How it works.** A glossary (term · what it means here · the code that embodies it), with a "cast of
     characters" table for names that are easy to confuse (`UserStore` vs `UserService` vs `UserManager`). The core
     entities: a ```mermaid stateDiagram of each one's lifecycle, and a short real sample of what it looks like on
     disk or in the database. Then one flow per entry point from step 3: a ```mermaid sequenceDiagram, then numbered
     stops in the code (what happens at each, and why there), and what happens when it fails.
   - **3. Patterns, naming, and why.** What makes code here look like it belongs:
     - The patterns: the shapes this code repeats, described in its own terms. Most aren't textbook patterns (MVC,
       repository, observer); name one only when it really fits, and then say how this code differs. For each: what
       it looks like (a short real snippet), where it is used, why, and how to write a new one.
     - Naming: how folders, files, types, functions, tests and config are named, the prefixes, suffixes and verbs
       used and what each signals (e.g. `get` reads memory, `load` reads disk), with real examples.
     - The design decisions, including the defensive ones (what edge case each guards against). With fewer than
       ~50 commits and no design docs, keep them to a paragraph.
   - **4. Making a change.** What I need to work in it:
     - Where to look: a table of common tasks, "To find or change … → look in … (named like …)".
     - Recipes for "add a new X", from the commits that did.
     - The house rules: layering, errors, logging, config, tests, code generation, build and CI; for each, an
       example and how it is enforced (a script, a lint, a CI step, or nothing).
     - What a change ripples into: the code most of the rest depends on, what changes together, and the trust
       boundaries a change must keep (auth, validation, sandboxing). State them as facts about how it is built.

   Keep it true:
   - Link every claim about the code to it, with the symbol as the link text: [`UserStore.create`](path/to/file.ts:42).
     Read the line before you link it.
   - Dependencies, callers and clusters come from the map and the graph, not from your impression of the code.
   - Describe what a script or mechanism actually does ("fills the marked regions in these files"), not a plausible
     summary ("copies them").
   - Tag what you didn't verify: _(inferred from …)_ when you reasoned it from the code or history, _(unknown)_ when
     there's no evidence. Never invent a reason. Ask me the unknown "why"s in the guide: "Why X? I found no record."
   - Explain, don't review: no findings, no severities, no "should". Where something is missing or surprising
     ("nothing checks that the client is regenerated"), say it plainly where it belongs in the guide and move on.
   - Every heading says what I learn by reading it. Plain language; no line-by-line walkthrough.
   - Keep Mermaid plain so it renders: short node ids, labels in double quotes, one diagram per fence; in a
     flowchart, no parentheses or slashes outside quotes. In a C4 diagram, give every element a description and
     every arrow a verb of at most five words and its protocol, keep it under 15 elements and 10 arrows, and end
     it with `UpdateLayoutConfig($c4ShapeInRow="3")`.

   End each chapter with one check, a prediction I can answer before I look further, such as "**Check:** where would
   you add a new MCP tool, and what else would you touch? Select this question and comment your answer." Don't
   write the answer on the page.

   Add it with `co_review_start({ reviewId, overview, open: false })`. With a focus, write the guide for that part,
   and say in chapter 1 how it fits the whole.
5. Show it: `co_review_start({ reviewId })`. It opens on the overview page, the guide, with **Done** instead of a
   verdict. Tell me it is open in Co-Review, with the chapters and how long they take to read, in one short message.
6. Then loop `co_review_wait` → investigate → `co_review_reply` until it says I closed it (I'm done: tell me in one
   line what's still unanswered, if anything). When I ask, answer from the code. When I answer a check, say whether it
   matches the code and why, with a link: no score, and say what I got right. When I say the guide is wrong, check
   the code: if it is, say so, fix the guide (`co_review_start({ reviewId, overview, open: false })`) and tell me what
   changed. Change no files. When I want its problems found, that is a review of its own, the audit command.
<!-- /prompt -->

Focus: $ARGUMENTS
