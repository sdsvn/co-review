The goal is that afterwards I can, without you: say what the repository does and where everything lives, use its
vocabulary, trace its main flows, recognise its patterns and naming, say why it is built this way, follow its house
rules, know where it is fragile, and place a change and predict what it affects. Familiarity is not the goal; being
able to predict is.

1. Prepare the review without showing it: `{{open_review}}({ title: "Understanding the repository", open: false })`.
   Keep the `reviewId` it returns. Don't tell me about it yet.
2. Read before you write: the README and docs, AGENTS.md / CLAUDE.md, the CI config, the entry points and the core
   types. Then get the facts Co-Review works out without a model: `{{repo_facts}}` for the summary, then each section
   (`entry`, `folders`, `load`, `names`, `shapes`, `history`; with a focus, pass it as `focus`). They are your
   evidence: the commit to pin the guide to, the entry points, every folder's role and why, the load-bearing files,
   the naming rules, the repeated shapes, the hotspots, what changes together, and the recipes from commits that
   created a file. Use `{{repo_map}}` to find definitions. {{> graphify}}
3. Everything the facts list goes in the guide; don't pick by taste. Each entry point gets a flow in chapter 2 or is
   named there as not traced (registrations of one kind, like the routes of one API, may share a flow). Each mapped
   folder gets a row in the map. Each naming rule, repeated shape and recipe is explained in chapter 3, or dismissed
   with a reason. A role marked `low` is a guess from imports: confirm or correct it from the code, and say which.
   When `history.thin` is true, say so instead of inferring from history.
4. Write the guide as the review's overview, four chapters (`##` headings), in this order:
   - **1. Lay of the land.** First line: "Written for `<short sha>` on <date>.", the facts' commit. Then what it does,
     for whom, and what it doesn't do; a ```mermaid context diagram (the system and what it talks to, from the
     outbound calls and environment in the facts); how to build, run, test and deploy it (the actual commands). Then
     the map: a row per folder in the facts' map, with its one-line job, its role (core, infra, glue, tooling, tests,
     generated, vendored) and its key files; the facts' ignore list in one "you can ignore" line.
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
     - Where to look: a table of common tasks, "To find or change … → look in … (named like …)".
     - The house rules: layering, errors, logging, config, tests, code generation, build and CI; for each, an
       example, how it is enforced (a script, a lint, a CI step, nothing) and what breaks when it is broken.
     - Recipes for "add a new X", from the commits that did.
     - The design decisions, including the defensive ones (what edge case each guards against). With fewer than
       ~50 commits and no design docs, keep the decisions and recipes to a paragraph; the rest stays.
   - **4. Where it hurts.** The load-bearing code (what most depends on), the hotspots (churn × size), what changes
     together, what is untested or dead, and the trust boundaries. Say in one line when there is too little history
     to tell. Then add your findings with `{{add_findings}}`: the area as the first label, `status: "proposed"`, at
     most three per area, each with what's wrong, why it matters and what to do, with `severity`, on the code (`path`
     and `code`, the line(s) copied exactly from the file), a folder (`path` alone) or the repository (`path: "."`).
     The page lists them after the guide.

   Keep it true:
   - Link every claim about the code to it, with the symbol as the link text: [`UserStore.create`](path/to/file.ts:42).
     Read the line before you link it.
   - Dependencies, callers and clusters come from the map and the graph, not from your impression of the code.
   - Describe what a script or mechanism actually does ("fills the marked regions in these files"), not a plausible
     summary ("copies them").
   - Tag what you didn't verify: _(inferred from …)_ when you reasoned it from the code or history, _(unknown)_ when
     there's no evidence. Never invent a reason. Ask me the unknown "why"s in the guide: "Why X? I found no record."
   - Every heading says what I learn by reading it. Plain language; no line-by-line walkthrough.
   - Keep Mermaid plain so it renders: short node ids, labels in double quotes, no parentheses or slashes outside
     quotes, one diagram per fence.

   End each chapter with one check, a prediction I can answer before I look further, such as "**Check:** where would
   you add a new MCP tool, and what else would you touch? Select this question and comment your answer." Don't
   write the answer on the page.

   Add it with `{{open_review}}({ reviewId, overview, open: false })`. With a focus, write the guide for that part,
   and say in chapter 1 how it fits the whole.
5. Show it: `{{open_review}}({ reviewId })`. It opens on the overview page: the guide, then your findings grouped by
   area, then the repository's areas. Tell me it is open in Co-Review, with the chapters and how long they take to
   read, in one short message.
6. Then loop `{{await_reviewer}}` → investigate → `{{reply}}` until it returns my Submit or says I closed the review
   (closing it means I'm done: act on nothing, and tell me in one line what's still open). When I answer a check,
   say whether it matches the code and why, with a link: no score, and say what I got right. When I say the guide
   is wrong, check the code: if it is, say so, fix the guide (`{{open_review}}({ reviewId, overview, open: false })`)
   and tell me what changed. On my Submit: approve → nothing to change unless I say so; request changes → fix what I
   accepted and asked for, as commits, then reply in each thread with what changed and wait again. Don't edit files
   while I read unless I ask in a thread.
