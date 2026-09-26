---
title: Review with Co-Review
description: Start a review, comment, ask the agent and submit.
sidebar:
  order: 4
---

## Start a review

The Review panel is where every review begins.
Open it from the status bar or with `Cmd+Shift+Alt+R`, then pick what to review:

- the **entire repository**
- the **current file**
- a **branch** against its base
- a **commit**

You can also right-click files or folders and choose **Start Review of Selection**.
When Claude Code opens a review, it appears on its own.

## Comment and discuss

Each target has its own gesture:

| To comment on | Do this |
|---|---|
| A line | Hover it and click **+** in the gutter |
| Several lines | Drag the **+**, or select them and press `Cmd+Alt+M` |
| A symbol | Right-click inside it then **Add Review Comment on Symbol** |
| A file or folder | Right-click it in the explorer then **Add Review Comment** |
| The repository | **...** in the Review panel then **Comment on the repository** |

Until an agent joins the review, everything is a plain comment.
Once one is connected, **Ask Agent** appears, and a multi-line selection defaults to asking.
To connect one yourself, use **...** then **Connect agent**, and pick its model and reasoning effort from the list it offers.

Threads sit inline under the code.
The panel lists them by file (**Open / Proposed / Resolved / All**).

## Review the whole repository

An **entire repository** review shows your progress as you read.

- **Coverage** in the Review panel: how many source files you've viewed, overall and per area. Click an area to open its next unviewed file.
- **Mark a file as viewed** with `Cmd+Alt+V`, the status bar item, or the editor's right-click menu. A file that changes afterwards counts as unviewed again.
- **The explorer** shows a check on viewed files and the number of open threads on files and folders.
- **Overview** (next to the coverage bar) opens a page with where to start, the areas of the code and how they connect. With a [Graphify](https://pypi.org/project/graphifyy/) graph (`graphify update .`, code only, no LLM) it shows the most connected code and a diagram of its clusters.
- **`/co-review:audit`** has Claude go first: proposed findings labelled by area. **By area** in the panel groups them.

## Designs, diagrams and patches

Rendered review view opens automatically for `index.markdown` and `*.pseudocode.md`.
For any other `.md` file, use **Open With then Review (rendered)**.

Inside the rendered view you can:

- Select text to comment, ask, or **Propose Edit**.
- Comment on a diagram or one of its nodes.
- Comment on any step of the design tree.

`.patch` and `.diff` files open as pull-request pages.

## Submit

When you are ready, click **Submit review** in the panel.
Choose Approve, Request changes or Comment, and add a message.
The agent gets every open comment and any accepted suggestions in one batch.

## From your phone

Open the mobile view with **... then Open mobile view**, or go to `http://<host>:<port>/m/`.

It accepts local connections only.
To use it over Tailscale, run `tailscale serve --bg 3000` and allow the host with `CO_REVIEW_ALLOWED_HOSTS`.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Cmd+Alt+M` | Comment on the line or selection |
| `Cmd+Alt+A` | Ask the agent about the line or selection |
| `Cmd+Alt+Down` / `Cmd+Alt+Up` | Next / previous comment |
| `Cmd+Alt+O` | Go to comment... |
| `Cmd+Alt+V` | Mark the file as viewed (or not) |
| `Cmd+Shift+Alt+R` | Show or hide the Review panel |
