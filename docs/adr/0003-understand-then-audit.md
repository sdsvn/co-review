# ADR 0003: Understand first, audit later: two whole-repository commands that don't overlap

- **Status:** Accepted
- **Date:** 2026-11-13
- **Supersedes:** [ADR 0002](0002-one-whole-repository-command.md)

## Context

ADR 0002 made `/co-review:audit` the one whole-repository command: a guide in four chapters whose last chapter,
"Where it hurts", carried the agent's proposed findings, with a Submit and a verdict. Used on a real repository, that
mix sent the wrong message. People open a repository review to understand it: what it does, where things live, how
it flows, how it is built. A page that ends in findings and asks for a verdict turns learning into judging, before the
reader knows enough to judge. Reviewing comes much later, once the repository is theirs again.

ADR 0002 rejected two commands because they overlapped: both wrote the guide, both listed the findings, and a person
had to choose between two ways of reading the same thing. Split by purpose instead of by depth, they no longer
overlap.

## Decision

1. **`/co-review:understand`** (`/co-review-understand` in Pi and Oh My Pi) explains the repository. The overview
   page is a guide in four chapters: the lay of the land, with a C4 context diagram and, when there is more than one
   running part, a C4 container diagram; how it works; patterns, naming and why; making a change. It opens with
   `explain: true`: no findings, no severities, no verdict, and the reader closes it with **Done**. The agent answers
   questions, checks the reader's answers to each chapter's question, and fixes the guide when it is wrong.
2. **`/co-review:audit`** (`/co-review-audit`) reviews the repository for problems, for someone who already knows it:
   a short overview (what it does, a C4 context diagram, the areas, where it hurts) and at most three proposed
   findings per area, then the reader's verdict. When the reader doesn't know the repository yet, it offers the
   guide first.
3. **The guide explains, it doesn't judge.** Something missing or surprising is stated as a fact where it belongs
   ("nothing checks that the client is regenerated"); finding it wrong is the audit's job.
4. Both start from the same evidence, the repository facts (`repo_facts`), and keep ADR 0001's rules for truth: every
   claim linked, unverified claims tagged, unknown reasons asked, never invented.

## Consequences

- Two commands again, but each says what it does, and neither repeats the other: the guide holds no findings, the
  audit holds no guide.
- `prompts/understand.md` is the guide; `prompts/audit.md` is the review. The skill, the commands, the MCP server's
  instructions and the docs name both.
- Someone who ran `/co-review:audit` to learn a repository now gets a short review instead of a guide; its first step
  tells them to start with `/co-review:understand`.

## Alternatives considered

- **Reshape `/co-review:audit` into the guide and drop its findings.** Rejected: "audit" says "find what's wrong",
  so the name would promise the opposite of what the command does.
- **Keep one command with findings optional.** Rejected: the reader still meets a verdict on a page meant for
  learning, and the agent still spends its first pass on judging.
