# Proposal: understanding a repository, not just auditing it

Status: accepted ([ADR 0001](../adr/0001-understanding-first-repository-review.md)); phase 1 implemented as
`/co-review:understand`. The method as language-neutral pseudocode, for porting:
[understanding-a-repository.pseudocode.md](understanding-a-repository.pseudocode.md).

`/co-review:audit` today is classical review: 4–10 areas, at most three risk findings each, an overview page, and
coverage as "files marked viewed". It tells you what's wrong, not what's there. After reading it you still can't say
what each folder is for, what the domain words mean, how a request flows, or what the house rules are. This proposal
makes understanding the front of the whole-repository review, and the findings one chapter of it.

## 1. What "understanding a repository" means

**Familiarity** is recognition: the names ring a bell and you know roughly where things are.
**Understanding** is recall, prediction and transfer: without the agent you can explain the repository, predict what
a change will do, and make a change that fits.

You understand a repository when you can do these nine things unaided. Each is checkable with a question.

| # | Ability | You can… | Grounding |
|---|---|---|---|
| U1 | Purpose & boundary | say what it does, for whom, what's out of scope; what it talks to; how to build, run, test and deploy it | Pennington's situation model |
| U2 | Map | give every important folder's job and role (core / infra / glue / tooling / generated / vendored / tests); know what to ignore; answer "where would X live?" | Storey's orientation; information foraging |
| U3 | Vocabulary & data | define the domain terms and name the code that embodies them; tell apart confusable names; know the core entities, their lifecycle, and what they look like on disk or in the database | Biggerstaff's concept assignment; Nam et al. 2024 |
| U4 | Mechanics | trace every main flow end to end; know the external contracts (APIs, protocols, file formats), the extension points, where and how state is persisted, concurrency, and how failures are handled | LaToza & Myers' reachability questions; Sillito et al. |
| U5 | Rationale | say why it's built this way, what it guards against, and what was traded off; know which "why"s are documented and which are guesses | Letovsky's "why" conjectures; Ko et al. |
| U6 | Patterns & conventions | recognise the shapes the code repeats (in its own terms, not only textbook patterns), how things are named and what the names signal, where to look for what; state the house rules (naming, layering, errors, logging, config, tests, codegen, build/CI), *how each is enforced and what breaks when it's violated*, and the recipe for "add a new X" | Soloway & Ehrlich's rules of discourse; NATURALIZE; Dagenais et al. |
| U7 | Health & risk | know the load-bearing and the fragile code, the hotspots, what's untested or dead, the trust boundaries | Tornhill; Bird et al. |
| U8 | Act | locate where a change goes, predict its blast radius, judge whether it fits | — |
| U9 | Currency | know whether your picture is still true: what changed since you last understood it | — |

U7 is what the audit produces today. The rest is what's missing.

## 2. The experience: four chapters, a check at the end of each

Top-down first, then bottom-up (von Mayrhauser & Vans). Each chapter ends with one optional prediction question,
posed as a thread: *"Before you look: where would you add a new MCP tool?"* Answer if you want; the agent says
whether that matches the code and why. No score. Reading LLM explanations passively creates an illusion of
competence (Prather et al. 2024); one prediction per chapter is the cheapest fix a developer will tolerate.

| Chapter | Abilities | Content |
|---|---|---|
| 1. Lay of the land | U1, U2 | Purpose, users, boundary; one context diagram; build/run/test/deploy commands; the annotated map: each folder's job, role, key files, depends-on / used-by; generated and vendored folders listed as "ignore" |
| 2. How it works | U3, U4 | Glossary (term → meaning → code → where used), with a "cast of characters" table for confusable names; core entities as lifecycle diagrams plus a real sample of their persisted form; one flow per entry point and external contract, each a sequence diagram with numbered stops in the code and a "what if this fails" note |
| 3. Patterns, naming, and why | U5, U6 | The patterns the code repeats, described in its own terms, with a real example each; naming (prefixes, suffixes, verbs and what they signal); a "to find or change X, look in Y" table; decisions tagged documented / inferred / unknown, including the defensive ones (what edge case each guards against); house rules, each with an example, its enforcement (script, lint, CI step, review) and what breaks without it; recipes for "add a new X" from past commits. **With fewer than ~50 commits and no design docs, decisions and recipes shrink to a paragraph**; patterns, naming and lookups stay, since they need only the code. |
| 4. Where it hurts | U7 | Load-bearing code, hotspots, change coupling, untested and dead code, trust boundaries; the audit's findings grouped by area. History sections say in one line when there isn't enough history. |

A short session is chapter 1 alone, and that is already more useful than today's audit. Each chapter can be entered
from anywhere: a concept, a folder, a flow, or "what changed recently".

**Reopening it later (U9).** The guide is pinned to the commit it was written for. On reopening: "since you last
read this: 34 commits; areas X and Y changed; these glossary terms and flows are affected", and only those parts are
refreshed. That's the original promise: months later, the codebase feels like yours again.

## 3. How the agent writes it, and keeps it true

1. **Read first**: README, docs, AGENTS.md / CLAUDE.md, manifests and build files, CI config, entry points, core
   types. (The pilot found this front-loading made every later page faster.)
2. **Bottom-up summaries** (Sun et al. 2025): file → folder → area → system, from the Tree-sitter map, plus the
   Graphify graph when it's installed. Graphify is never required.
3. **Feature cross-cut** (RepoSummary, 2025): the map is by folder; glossary and flows are by feature and span folders.
4. **Coverage of flows is checked, not chosen by taste**: list the entry points and external contracts (MCP tools,
   HTTP routes, CLI commands, jobs, outbound integrations) and make sure each is covered by a flow or named as
   not traced.
5. **Structure comes from tools, not prose**: dependency direction, callers and clusters come from Tree-sitter or
   Graphify; the model explains them, it doesn't assert them.
6. **Every claim is tagged**: *verified*, with a symbol reference (`path` + `Symbol`, resolved to a line by
   Co-Review the way findings' quotes are); *inferred*, saying from what; or *unknown*. An unknown "why" becomes a
   question to you, never a made-up story.
7. **Mechanisms, not paraphrase**: for a script or convention, describe what it actually does (e.g. "fills marked
   regions inside these files"), not a plausible summary ("copies to").
8. **Information scent** (Piorkowski et al.): every heading and link says what you'll learn by following it.
9. **Your corrections feed back**: "that's not how it works" in a thread makes the agent re-check the code and fix
   the guide.

## 4. Where it shows up in Co-Review

**Phase 1, with what exists today:**

- The repository review's **overview page** becomes the guide's front: chapter 1 in full, then links to chapters 2–4
  as commentable pages (Markdown and Mermaid, symbol links open the code).
- **Checks** and **corrections** are threads, answered by the agent.
- **Findings** stay as they are and are shown in chapter 4.
- Everything is stored outside the repository: `~/.co-review/workspaces/<hash>/guide/<commit>/`.

**Phase 2, needs app work:**

- **Explorer**: each folder shows its one-line job and role; generated and vendored folders are dimmed. That is the
  map, where you already look.
- **Hover cards**: a glossary term in code shows its definition and where it's defined.
- **Tours**: a flow's stops step through the editor (next/prev), each stop saying what happens there and why.
- **Progress** per ability and area (seen → read → checked), next to the per-file coverage.
- **Your model vs the code** (reflexion models, Murphy et al.): you state how you think the parts depend on each
  other, and Co-Review shows where the dependency graph agrees and where it doesn't.

**Phase 3:** currency: commit pinning, "since you last read this", partial refresh.

## 5. Rollout and how to know it worked

1. **Prompt-only**: a new `prompts/understand.md` with `/co-review:understand`, and `/co-review:audit` starting from it.
   Try it on three repositories of different size and age, including one that's old and multi-author.
2. Phase 2 UI, then phase 3 currency.

Measure ability, not satisfaction: do people answer the chapter checks correctly without looking; do they correct
the guide (a truthfulness signal); do they stop at chapter 1 or keep going; and after changes, does "since you last
read this" name what actually changed. Don't count completion rates or "was this helpful".

## 6. Evidence so far: a pilot on Co-Review itself

An agent wrote the guide for this repository following the method (v1, eight pages). A second agent that had never
seen the code read *only* the guide and answered 26 questions closed-book: 12 the guide's author wrote and 14 held-out
questions written independently by an agent with code access. A third agent graded them against the code.

- **20 correct, 6 partial, 0 wrong.** Guide questions: 11 correct, 1 partial. Held-out: 9 correct, 5 partial.
- **Perfect on U2 map, U3 vocabulary, U7 health, U8 act.** Weakest: **U6 conventions** (all three partial), then one
  partial each on U1, U4, U5.
- **Guide accuracy:** 13 of 15 sampled claims accurate, 2 partly accurate, none wrong. One partly accurate claim
  ("copies to plugin/skills/") made the learner confidently half-wrong, which is why point 7 in section 3 exists.
- The learner reported the annotated map and the end-to-end flows taught it the most, and that it would feel able to
  make a scoped backend change, but not front-end or GitHub-posting changes without reading the source.

The gaps map to v3 changes: a flow per entry point (the GitHub posting flow was missing), conventions with their
enforcement and failure, defensive decisions in rationale, a "cast of characters" for confusable names, and a
sample of persisted data.

Caveat: the learner was a model, not a person. A model starts with general knowledge a newcomer may lack, and reads
everything diligently. The phase 1 trial with people is the real test.

## References

- Brooks 1983, comprehension via hypotheses and beacons (*IJMMS*).
- Soloway & Ehrlich 1984, programming plans and rules of discourse (*IEEE TSE*).
- Pennington 1987, program model vs situation model (*Cognitive Psychology*).
- Letovsky 1987, why/how/what conjectures (*JSS*).
- von Mayrhauser & Vans 1995, integrated metamodel (*IEEE Computer*).
- Storey 2006, theories and tools of program comprehension (*Software Quality Journal*).
- Sillito, Murphy & De Volder 2006, questions programmers ask (*FSE*).
- Ko, DeLine & Venolia 2007, information needs (*ICSE*).
- LaToza & Myers 2010, reachability questions (*ICSE*).
- Piorkowski, Fleming et al. 2013, information foraging in software (*CHI*; *TSE*).
- Dagenais et al. 2010, newcomers moving into a project landscape (*ICSE*).
- Steinmacher et al. 2015, barriers for newcomers (*IST*).
- Murphy, Notkin & Sullivan 1995/2001, software reflexion models (*FSE*; *TSE*).
- Biggerstaff 1994, the concept assignment problem (*ICSE*).
- Allamanis et al. 2014, NATURALIZE: learning coding conventions (*FSE*). https://arxiv.org/abs/1402.4182
- Tornhill 2015, *Your Code as a Crime Scene*; Bird et al. 2011, ownership and quality (*FSE*).
- Nam et al. 2024, an LLM to help with code understanding (*ICSE*). https://arxiv.org/abs/2307.08177
- Luo et al. 2024, RepoAgent. https://arxiv.org/abs/2402.16667
- Yang et al. 2025, DocAgent. https://arxiv.org/abs/2504.08725
- Sun et al. 2025, hierarchical module summarization. https://arxiv.org/abs/2503.10737
- RepoSummary 2025, feature-oriented summarization. https://arxiv.org/abs/2510.11039
- SWE-QA 2025, repository-level question answering. https://arxiv.org/abs/2509.14635
- Prather et al. 2024, LLM use and the illusion of competence.
