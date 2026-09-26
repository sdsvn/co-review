---
title: Claude Code
description: Install the Co-Review plugin for Claude Code.
sidebar:
  order: 3
---

One plugin install makes Claude Code a co-reviewer.

First, [install the `co-review` command](/co-review/docs/start/install/) -- the plugin needs it to launch `co-review mcp`.

Then install the plugin. Inside Claude Code, run:

```bash
/plugin marketplace add sdsvn/co-review
/plugin install co-review@co-review
```

The first command adds Co-Review's plugin catalog.
The second installs the plugin from it.

From a terminal, as one line:

```bash
claude plugin marketplace add sdsvn/co-review && \
  claude plugin install co-review@co-review
```

Check it with `/mcp` -- `plugin:co-review:co-review` should be connected.

## What you get

The plugin gives you several ways to start and run a review:

- **`/co-review:review [focus]`** -- Claude opens its change in Co-Review, flags what deserves attention, answers your comments in their threads, and acts on your verdict.
- **`/co-review:design <task>`** -- Claude writes a design doc, you review it, and it implements what you approved.
- **`/co-review:audit [focus]`** -- Claude reviews the whole repository first: a few proposed findings per area, for you to accept or dismiss, then it answers your questions.
- **Live comments** -- With channels on, your questions reach the running session the moment you ask them.
- **`co-review:co-reviewer`** -- A background subagent that keeps answering your review while the main conversation works.
- **Session hook** -- Opening Claude Code in a repository with open reviews tells it which questions are waiting.
- **Skills** -- "Review this with me" and "design this first" work as plain requests.

## Live comments

Channels are a Claude Code research preview.
To enable them, start Claude Code with a flag:

```bash
claude \
  --dangerously-load-development-channels \
  plugin:co-review@co-review
```

Without the flag, everything still works.
Claude waits for your comments instead of receiving them as they happen.

## For a team

To offer the plugin to everyone on the project, commit this to `.claude/settings.json`:

```json
{
  "extraKnownMarketplaces": {
    "co-review": {
      "source": {
        "source": "github",
        "repo": "sdsvn/co-review"
      }
    }
  },
  "enabledPlugins": {
    "co-review@co-review": true
  }
}
```

Claude Code will offer the plugin when someone trusts the folder.

For Pi, Codex, Cursor, VS Code, Gemini CLI, Zed and others, see [Connect your agent](/co-review/docs/guides/connect-your-agent/).
