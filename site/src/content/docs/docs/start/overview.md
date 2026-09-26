---
title: What is Co-Review?
description: Review your entire repository with the coding agent that built it, and know your codebase again.
sidebar:
  order: 1
---

Building with a coding agent is fast. Change by change, each diff looks fine, and months later the codebase works but no longer feels like yours: you don't know where things live or why they're built the way they are.

Co-Review is for getting it back. Once in a while, open the **entire repository** as a review and read it with the agent that built it, the way you would read a pull request:

- an overview shows where to start and how the areas of the code connect;
- the agent can take a first pass, with proposed findings grouped by area;
- you comment on a line, a function, a file or a folder, and ask why, and the agent answers in the thread, from the code;
- coverage tracks which files you've read, per area.

The same review works for everything in between: a single change, a branch against its base, a commit, or a design before any code exists. The agent answers in the thread, with links into the code, and waits for your verdict before it changes anything.

Co-Review is a review tool with IDE features, not an IDE. It is built on [Eclipse Theia](https://theia-ide.org), so code navigation, search, Git and language servers are the real thing.
Everything unrelated to reviewing is removed.

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
