---
name: co-review-design
description: Design a non-trivial change before coding and have the human review it in Co-Review. Investigate the code, write the smallest design document that lets them understand, challenge and approve the approach (Context, a Design step tree of what → how → what can go wrong, optional Alternatives / Behaviour / Open Questions), open it in Co-Review, revise it from their comments, and implement only after approval. Use when the user says "design this first", "let me review the design", "write a design doc", "plan before coding", or when a change is architecturally non-trivial (new component, data model, API, async flow, migration). Needs Co-Review's MCP tools.
---

# Design first, reviewed in Co-Review

**A design document is the smallest document that lets another engineer understand, challenge and approve the
change.** It exists for review: show the decisions, assumptions, trade-offs, important behaviour, failure modes and
open questions, and nothing else. Full method and examples: Co-Review's `docs/design-docs.md`.

> **In Pi or Oh My Pi** (the Co-Review package) the tools are `co_review_start` (open_review; `dir` for a review
> directory), `co_review_wait` (await_comment), `co_review_reply`, `co_review_ask` and `co_review_verdict`
> (await_review). In an interactive session, the reviewer's comments also arrive on their own as `[Co-Review]`
> messages.

## 1. Investigate before designing

- Read the code the change touches first: the source, abstractions, APIs, data models, tests, infrastructure, event
  flows. Find how the repository already solves similar problems.
- **Prefer existing patterns.** Don't add a new abstraction, dependency, service, store, protocol or infrastructure
  when an existing one does the job. If something new is needed, make it an explicit Design step.
- **Don't invent** requirements, constraints, traffic numbers, SLAs, guarantees or business rules. If missing
  information would change the design, ask it under Open Questions.
- Choose the simplest design that solves the actual problem. No speculative scalability, configurability,
  extensibility or abstraction.
- Think through requirements, compatibility, security, performance, observability, migration, testing, rollout and
  failure handling, but only write down what materially affects the design, where it belongs.

## 2. Write the document

`design/<short-name>/index.markdown` in the repository (or a scratch directory if the user doesn't want design docs
committed):

````markdown
---
co-review: design
---
# <The change, as a short imperative>

## Context
Why are we changing this, what happens today, why it must change. Only what the reviewer needs.

## Design
- <L1 what: a responsibility or behaviour that matters from outside>
  - <L2 how: components, data flow, APIs, persistence, the pattern reused>
    - <L3 what can go wrong: failure, retry, concurrency, consistency, edge case, security, compatibility>

## Alternatives
Optional. Only a real choice a reviewer may challenge: each option's trade-off, and why this one.

## Behaviour
Optional. Runtime scenarios not obvious from the Design (failure, retry, crash, race). Mermaid welcome.

## Open Questions
Optional. `- ` items: genuinely unresolved decisions that need the reviewer.
````

- **Only Design is required.** Leave out optional sections that don't help. Don't add others: a migration, a
  compatibility constraint or a security boundary goes where it matters, usually as a Design step.
- **Size follows complexity**: about 10–30 lines for a small change, 30–80 for a medium one, 80–150 for a large one,
  more only when the complexity justifies it. Never pad; never drop a decision or failure mode to stay short.
- **What before how.** An L1 states behaviour ("Deliver each webhook at least once"), not technology ("Use Kafka").
  Add L3 only where it matters.
- **Be concrete.** "Retry server errors with exponential backoff", not "handle transient failures appropriately".
  One decision per step, a line or two long; details go in child steps. Don't restate the task.
- **A person reads this, not a compiler.** Plain language. Name code once, at the top level (the entry point, module
  or new table a step is about), then refer to it in words. **No line numbers, no `file:line` links, no pasted
  code** (Mermaid is fine). Those belong in review threads and in the implementation.
- Format: `## Design` is one nested `- ` list, 2 spaces per level (not `*`, `+` or `1.`). An optional short reason
  goes after ` — `. Optional markers in backticks: `?` open question, `⚠` risk, `✎` new name. Mermaid blocks start
  with `%% id: <name>` and use explicit node ids (`api[CreateOrder]`).

## 3. Self-review, then open it

Check: is the problem clear; are the important decisions and trade-offs visible; is what separate from how; are the
failure modes that matter covered; were existing patterns used; is anything more complex than needed; are the open
questions real; can a person follow it without opening the code; is the size right? **Remove anything that doesn't
help the reviewer understand or decide.**

Then `open_review({ dir: "<absolute path to design/<short-name>>" })`. Co-Review checks the document and returns
problems in `format.warnings`: the structure, line references, pasted code, overlong steps, and code names repeated
across steps. Fix every warning and open it again; share the `url` only when the list is empty.

## 4. Review loop

Follow the `co-review` skill's loop (`await_comment` → investigate → `reply`). For each comment:

- Check the code if needed, then decide what the comment points at: missing information, a wrong assumption, an
  architectural problem, an open decision, or a detail that doesn't belong. Update the document and reply saying
  which step changed.
- Don't accept suggestions blindly: if one conflicts with the code, say so, with the evidence.
- **Keep commented steps' wording stable.** Comments anchor to a step's text, so rewording outdates them, and moving
  doesn't. Prefer adding or restructuring steps around a commented one. If a step is wrong, fix it anyway.
- Move settled Open Questions into the Design.

## 5. After the verdict

`await_review` returns the decision:

- **approve**: implement the approved design, then check the code against it. If implementation shows the design is
  wrong or incomplete, stop that part, update the design, and get it approved again before continuing. Don't
  silently change the architecture. A detail that doesn't change a decision needs no new round.
- **request-changes**: revise the document, reply per thread, and wait for the next round.
- `acceptedSuggestions` on the document are already written into it by Co-Review (`doc.version` goes up). Re-read the
  file before editing.
