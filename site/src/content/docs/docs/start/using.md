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

## Review a whole repository

An **entire repository** review shows your progress as you read. Reach for it to get a codebase back in your head, or to check your own work.

- **Coverage** in the Review panel: how many source files you've viewed, overall and per area. Click an area to open its next unviewed file.
- **Mark a file as viewed** with `Cmd+Alt+V`, the status bar item, or the editor's right-click menu. A file that changes afterwards counts as unviewed again.
- **The explorer** shows a check on viewed files and the number of open threads on files and folders.
- **Overview** (next to the coverage bar) opens a page with where to start, the areas of the code and how they connect. With a [Graphify](https://pypi.org/project/graphifyy/) graph (`graphify update .`, code only, no LLM) it shows the most connected code and a diagram of its clusters.
- **`/co-review:audit`** has the agent go first: proposed findings labelled by area. **By area** in the panel groups them.

## Review a change or pull request

Between repository reviews, review one change at a time — the way you'd review a pull request.

- Pick a **branch** against its base, or a single **commit**, when you start the review (above). Or let the agent open the change for you: `/co-review:review` in Claude Code, `/co-review` in Pi.
- A **`.patch` or `.diff` file** opens as a pull-request page, with line comments and suggested edits.
- Findings the agent proposes appear as **Proposed** (Accept / Dismiss). Your verdict returns every open comment and any accepted suggestions in one batch, and the agent commits accepted edits on the branch.
- **Someone else's pull request?** Co-Review reviews local code, so bring the change to your machine first: check out its branch (`gh pr checkout <number>`) and review the branch against its base, or save the diff and open it as a patch — `gh pr diff <number> > pr.patch`, then open `pr.patch`.

## Design before the code

Review the plan before any code exists. A design document is **not source code** — it's a plan: prose, Mermaid diagrams and a foldable **L1 · L2 · L3** step tree (what happens, then how, then the edge cases), where every step, diagram node and line of text can carry a comment.

Rendered review opens automatically for `index.markdown` and `*.pseudocode.md` — the filename only triggers the render; the content is a design plan, not pseudocode. For any other `.md` file, use **Open With → Review (rendered)**.

Inside the rendered view you can:

- Fold the tree to L1, L2 or L3.
- Comment on any step of the design tree.
- Comment on a diagram or one of its nodes.
- Select text to comment, ask, or **Propose Edit**.

Have the agent write and revise the design for you: `/co-review:design <task>` (Claude Code) or `/co-review-design <task>` (Pi). It writes the plan, you review it, and it builds only what you approve. Format and details: [Design documents](/co-review/docs/guides/design-docs/).

## Submit

When you are ready, click **Submit review** in the panel.
Choose Approve, Request changes or Comment, and add a message.
The agent gets every open comment and any accepted suggestions in one batch.

## Finish a review

The dropdown at the top of the panel switches between reviews. When one is done, **... then Archive review** takes it out of the dropdown while keeping it on disk — reopen it later with **... then Show archived reviews...**. Delete review removes it for good.

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
