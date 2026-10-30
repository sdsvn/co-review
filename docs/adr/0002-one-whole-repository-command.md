# ADR 0002: One whole-repository command: `/co-review:audit` is the guide

- **Status:** Accepted
- **Date:** 2026-10-30
- **Supersedes:** decisions 2 and 5 of [ADR 0001](0001-understanding-first-repository-review.md)

## Context

ADR 0001 added `/co-review:understand`, which writes a guide to the repository, and kept `/co-review:audit` as the
quick findings-first pass, so neither command surprised the other's users. With both, a person had to choose between
two ways of reading the same repository before knowing which they needed, and the two prompts described the same
findings twice. The guide's last chapter, "Where it hurts", already holds the audit's findings by area, and since
the repository facts (`repo_facts`) it holds them on better evidence: real hotspots, change coupling and load-bearing
code.

## Decision

1. **There is one whole-repository command, `/co-review:audit`** (`/co-review-audit` in Pi and Oh My Pi). It writes
   the guide in four chapters, with the proposed findings by area in the last one. `/co-review:understand` is
   removed; "audit" is the name people already know.
2. **The agent says first that it takes a few minutes**, before it starts reading, and says again when the review is
   open in Co-Review.
3. **Only the risks?** The reader goes to "Where it hurts"; a narrower audit is a focus
   (`/co-review:audit internal/billing`), not another mode.

## Consequences

- An audit takes the agent a few minutes instead of one or two, and its overview page is a guide, not a summary of
  findings. Someone who only wants the findings reads the last chapter, or the panel's **Proposed** list.
- One prompt (`prompts/audit.md`) to keep true, and one structure for the checks the engine design adds
  ([understanding-engine.md](../proposals/understanding-engine.md)).

## Alternatives considered

- **Keep both commands.** Rejected: two modes to choose between and two prompts to keep in step.
- **Keep both names, with audit as a short mode of understand.** Rejected for the same reason: still two modes.
- **Keep only `/co-review:understand`.** It says better what the command does, but `/co-review:audit` is the command
  people already use.
