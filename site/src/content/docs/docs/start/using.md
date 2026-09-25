---
title: Review with Co-Review
description: Start a review, comment, ask the agent and submit.
sidebar:
  order: 4
---

## Start a review

Open the **Review** panel from the status bar or with `Cmd+Shift+Alt+R`, and pick what to review: the **entire
repository**, the **current file**, a **branch** against its base, or a **commit**. Right-click files or folders →
**Start Review of Selection** also works. When Claude Code opens a review, it appears on its own.

## Comment and discuss

| To comment on | Do this |
|---|---|
| A line | Hover it and click **+** in the gutter |
| Several lines | Drag the **+**, or select them and press `Cmd+Alt+M` |
| A symbol | Right-click inside it → **Add Review Comment on Symbol** |
| A file or folder | Right-click it in the explorer → **Add Review Comment** |
| The repository | **⋯** in the Review panel → **Comment on the repository** |

Until an agent joins the review, everything is a plain comment. Once one is connected, **Ask Agent** appears, and a
multi-line selection defaults to asking. Threads sit inline under the code; the panel lists them by file
(**Open / Proposed / Resolved / All**).

## Designs, diagrams and patches

`index.markdown` and `*.pseudocode.md` open in the rendered review view; any other `.md` through **Open With →
Review (rendered)**. Select text to comment, ask or **Propose Edit**; comment on a diagram or one of its nodes; or
on any step of the design tree. `*.patch` and `*.diff` files open as pull-request pages.

## Submit

**Submit review** in the panel: Approve, Request changes or Comment, with a message. The agent gets every open
comment and any accepted suggestions in one batch.

## From your phone

**⋯ → Open mobile view**, or `http://<host>:<port>/m/`. It accepts local connections only; to use it over
Tailscale, run `tailscale serve --bg 3000` and allow the host with `CO_REVIEW_ALLOWED_HOSTS`.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Cmd+Alt+M` | Comment on the line or selection |
| `Cmd+Alt+A` | Ask the agent about the line or selection |
| `Cmd+Alt+↓` / `Cmd+Alt+↑` | Next / previous comment |
| `Cmd+Alt+O` | Go to comment… |
| `Cmd+Shift+Alt+R` | Show or hide the Review panel |
