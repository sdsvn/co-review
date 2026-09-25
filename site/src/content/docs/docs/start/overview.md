---
title: What is Co-Review?
description: A review tool with IDE features, built for reviewing what Claude Code builds.
sidebar:
  order: 1
---

Co-Review is a review tool with IDE features, not an IDE. When Claude Code finishes a change, it opens the change in
Co-Review, and you review it the way you would a pull request. It isn't limited to diffs: you can review the entire
repository, a folder, a branch against its base, or a single commit. You comment on lines, functions, designs and diagrams, and
ask about anything you're reading. The agent that did the work answers in the same thread, with links into the code,
and waits for your verdict before it changes anything.

It's built on [Eclipse Theia](https://theia-ide.org), so code navigation, search, Git and language servers are the
real thing. Everything unrelated to reviewing is removed.

## What you can review

- **Code**: a line, a range, a function, a file, a folder or the whole repository. Comments are anchored to the code's
  symbol and Tree-sitter tokens, so they follow it through edits and moves.
- **Designs**: rendered Markdown with a foldable L1 · L2 · L3 design tree, where every step can take a comment. See
  [Design documents](/co-review/docs/guides/design-docs/).
- **Diagrams**: Mermaid diagrams, down to a single node or edge.
- **Patches**: `.patch` and `.diff` files as pull-request pages.

## How a review goes

1. Claude Code opens its change in Co-Review and flags what deserves attention, as *proposed* findings.
2. You read and comment. Select a few lines and **Ask Agent**, or leave a plain comment.
3. The agent answers in the thread, from its own context: it knows what it built and why.
4. You **Submit review**: Approve, Request changes or Comment, with a message. The agent gets every open comment,
   applies accepted suggestions, and comes back for another round.

## Next

- [Install Co-Review](/co-review/docs/start/install/)
- [Set up Claude Code](/co-review/docs/start/claude-code/)
- [Review with it](/co-review/docs/start/using/)
