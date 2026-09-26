---
title: What is Co-Review?
description: Review your entire repository with the coding agent that built it, and know your codebase again.
sidebar:
  order: 1
---

Building with a coding agent is fast. Change by change, each diff looks fine, and months later the codebase works but no longer feels like yours: you don't know where things live or why they're built the way they are.

Co-Review is a review tool with IDE features — not an IDE. You open something as a review, comment on a line, a diagram node or a design step, and the agent that did the work answers in the thread, from the code, and waits for your verdict before it changes anything. It is built on [Eclipse Theia](https://theia-ide.org), so code navigation, search, Git and language servers are the real thing; everything unrelated to reviewing is removed.

## Three ways to review

Co-Review is not only for whole-repository reviews. The same tool covers three jobs:

- **A whole repository** — open the entire codebase as one review to get it back in your head, or to check your own work. An overview shows where to start and how the areas connect, the agent can take a first pass with findings grouped by area, and coverage tracks which files you've read. See [Review a whole repository](/co-review/docs/start/using/#review-a-whole-repository).
- **A change or pull request** — a single change, a branch against its base, a commit, or a `.patch` / `.diff` file opened as a pull-request page, with line comments, suggested edits and one verdict. See [Review a change or pull request](/co-review/docs/start/using/#review-a-change-or-pull-request).
- **A design, before the code** — review the plan, not the source: a design written as a foldable L1 · L2 · L3 tree with Mermaid diagrams, where the agent revises from your comments and only builds what you approve. See [Design before the code](/co-review/docs/start/using/#design-before-the-code) and [Design documents](/co-review/docs/guides/design-docs/).

## What you can review

- **Code** -- a line, a range, a function, a file, a folder or the whole repository.
  Comments are anchored to the code's symbol and Tree-sitter tokens, so they follow it through edits and moves.
- **Designs** -- rendered Markdown with a foldable L1 / L2 / L3 design tree, where every step can take a comment.
  See [Design documents](/co-review/docs/guides/design-docs/).
- **Diagrams** -- Mermaid diagrams, down to a single node or edge.
- **Patches** -- `.patch` and `.diff` files as pull-request pages.

## How a review goes

1. Claude Code opens its change in Co-Review and flags what deserves attention as *proposed* findings.
2. You read and comment. Select a few lines and **Ask Agent**, or leave a plain comment.
3. The agent answers in the thread from its own context -- it knows what it built and why.
4. You **Submit review**: Approve, Request changes or Comment, with a message.
   The agent gets every open comment, applies accepted suggestions, and comes back for another round.

## Next

- [Install Co-Review](/co-review/docs/start/install/)
- [Set up Claude Code](/co-review/docs/start/claude-code/)
- [Review with it](/co-review/docs/start/using/)
