---
title: Claude Code
description: Install the Co-Review plugin for Claude Code.
sidebar:
  order: 3
---

One plugin install makes Claude Code a co-reviewer:

```bash
/plugin marketplace add sdsvn/co-review
/plugin install co-review@co-review
```

The first command adds Co-Review's plugin catalog, the second installs the plugin from it. From a terminal, as one
line:

```bash
claude plugin marketplace add sdsvn/co-review && claude plugin install co-review@co-review
```

The plugin launches `co-review mcp`, so [install the `co-review` command](/co-review/docs/start/install/) first.

## What you get

| | |
|---|---|
| `/co-review:review [focus]` | Claude opens its change in Co-Review, flags what deserves attention, answers your comments in their threads, and acts on your verdict |
| `/co-review:design <task>` | Claude writes a design doc, you review it, and it implements what you approved |
| Live comments | With channels on, your questions reach the running session the moment you ask them |
| `co-review:co-reviewer` | A background subagent that keeps answering your review while the main conversation works |
| Session hook | Opening Claude Code in a repository with open reviews tells it which questions are waiting |
| Skills | "Review this with me" and "design this first" work as plain requests |

Check it with `/mcp`: `plugin:co-review:co-review` should be connected.

## Live comments

Channels are a Claude Code research preview. Start Claude Code with:

```bash
claude --dangerously-load-development-channels plugin:co-review@co-review
```

Without the flag, everything still works: Claude waits for your comments instead of receiving them as they happen.

## For a team

Commit this to the project's `.claude/settings.json`, and Claude Code offers the plugin when someone trusts the
folder:

```json
{
  "extraKnownMarketplaces": { "co-review": { "source": { "source": "github", "repo": "sdsvn/co-review" } } },
  "enabledPlugins": { "co-review@co-review": true }
}
```

Pi, Codex, Cursor, VS Code, Gemini CLI, Zed and others: see
[Connect your agent](/co-review/docs/guides/connect-your-agent/).
