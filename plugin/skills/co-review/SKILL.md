---
name: co-review
description: Review with the human in Co-Review and stay as their co-reviewer — the whole repository, your change, someone else's pull request or a design: prepare the review, add first-pass findings, show it, answer their questions in the threads, and act on their verdict. Use when the user says "review this with me", "open a review", "co-review", "let me review your change", "walk me through the change", "I want to review before you continue", "review this PR", "audit the repo", or when a change is ready and the human should look at it before it is committed or merged. Needs Co-Review's MCP tools (open_review, await_reviewer, reply, add_findings, get_review).
---

# Co-review with the human

Co-Review is a review app where the human comments on code, designs and patches inline, and you answer in
the same threads. You are the co-reviewer: you did (or know) the work, so you explain and defend it, and you
change nothing until the human decides.

In Claude Code the tools come with the Co-Review plugin (`/plugin install co-review@co-review`) or
`claude mcp add -s user co-review -- co-review mcp`. If none are available, tell the user how to add them and stop.

<!-- prompt: pi-tools -->
> **In Pi or Oh My Pi** (the Co-Review package) the tools are named `co_review_start` (open_review; `dir` for a
> review directory), `co_review_wait` (await_reviewer: questions, answers and the Submit), `co_review_reply`,
> `co_review_add_findings`, `co_review_ask`, `co_review_map` (repo_map) and `co_review_post_github`
> (post_review_to_github). In an interactive session, the reviewer's questions also arrive on their own as
> `[Co-Review]` messages.
<!-- /prompt -->

## 1. Pick the workflow

| The human wants to review | Open it as | Claude Code command | Pi / Oh My Pi |
|---|---|---|---|
| The whole repository | `root` (the repository) | `/co-review:audit` | `/co-review-audit` |
| Your change (its diff) | a patch in a review directory | `/co-review:review` | `/co-review-change` |
| Someone else's pull request | the PR's patch + `PR.md` in a review directory | `/co-review:pr <n>` | `/co-review-pr <n>` |
| A design, before the code | a design document in a review directory | `/co-review:design <task>` | `/co-review-design <task>` |

Every workflow has the same shape: **prepare the review without showing it, do your first pass, then show it**, so
the human starts from your findings instead of an empty review, and never sees a half-built one:

1. `open_review({ ..., open: false })` creates (or joins) the review and returns its `reviewId`. Nothing is shown.
2. Your first pass: `add_findings`, and your overview for the review's front page (`open_review({ reviewId,
   overview, open: false })`: what it does and why, how it works as a Mermaid flowchart, the blast radius, where to
   start); for a design, fix every `format.warnings` entry instead (the document is its own front page).
3. `open_review({ reviewId })` shows it, with the Review panel (in the browser, or the repository's window in the
   desktop app). A repository, diff or pull-request review opens on its **overview page**: the pull request, your
   overview, the files changed, then every thread grouped by area, and for a repository its areas, as one rendered
   page the human reads and comments on (comments there come back with `target: "overview"` and the quoted text as
   `source`). A design opens on its document, with your findings inline. Give the human the returned `url`, or say it is open in the desktop app
   if there is a `note` instead, with a short summary.

Pass `title` to start a new review instead of joining the latest one.

### The whole repository

For getting a codebase back in your head, or checking it before a release. Start from a map of what matters
instead of 5,000 files:

<!-- prompt: audit -->
1. Prepare the review without showing it: `open_review({ title: "Repository review", open: false })`. Keep the
   `reviewId` it returns. Don't give me a URL yet.
2. Call `repo_map({ overview: true })` and `repo_map`. With [Graphify](https://graphify.net) installed, the
   overview is built from its code graph (Co-Review builds it: calls, imports, clusters, the most connected code), and
   `graphify-out/GRAPH_REPORT.md` lists import cycles and surprising connections between distant parts of the code:
   good leads. Split the repository (or the focus I gave you) into 4–10 areas and read the riskiest code in each
   (entry points, input handling, money, auth, concurrency, persistence).
3. Add findings with `add_findings`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do, with `severity`. A finding goes on the code (`path`,
   `line`); one about a whole area on its folder (`path` without `line`); one about the repository on `path: "."`.
   Say so in the overview when an area looks fine.
   Then write your overview of it for the review's front page, which I read first, and add it with
   `open_review({ reviewId, overview, open: false })`: what it does and why (a few sentences), how it works as a
   ```mermaid flowchart (the path a request or the data takes, 5–12 nodes), what else it affects (the blast radius:
   callers, data, configuration, other services), and where to start reading. Plain language; no line-by-line
   walkthrough.
4. Show it: `open_review({ reviewId })`. It opens on the overview page: your overview, every finding grouped by
   area, and the repository's areas, as one page I can read and comment on. Give me the URL (if there is one) and the
   areas with their finding counts, in one short message.
5. Then loop `await_reviewer` → investigate → `reply` until it returns my Submit or says I closed the review
   (tell me what's still open). On my Submit: approve → nothing to change unless I say so; request changes → fix
   what I accepted and asked for, as commits, then reply in each thread with what changed and wait again. Don't
   edit files while I review unless I ask in a thread.
<!-- /prompt -->

`repo_map({ overview: true })` says where to start and how the code clusters. When the `graphify` command is
installed, Co-Review builds a [Graphify](https://graphify.net) graph of the code first (Tree-sitter, no LLM, seconds;
in `graphify-out/`, kept out of `git status`), and while answering, `graphify query "<question>"`, `graphify path "A"
"B"` and `graphify explain "X"` answer how parts of the code connect without searching. A finding about a whole area goes on its folder (`path` without `line`), one about the
repository on `path: "."`; the overview page and the panel's **By area** view group findings by their first label, so
make that label the area.

### Your change

For a change you made, before it is committed or merged. The human reads the diff as a pull-request page, comments
on lines, suggests edits and submits one verdict:

<!-- prompt: review-change -->
1. Prepare the review without showing it: `open_review({ diff: "HEAD", patchName: "<short-slug>", title: "<what
   the change does>", open: false })` for uncommitted work (new files included), or `diff: "<base>...HEAD"` for a
   branch (`<base>` is the branch it will merge into, usually `main`). Co-Review runs the diff itself and makes it a
   pull-request page. Keep the `reviewId` it returns. If it says the diff is empty, tell me and stop.
2. Add findings only for real risks and non-obvious decisions, at most five, on the changed lines:
   `add_findings({ findings: [{ target: "patch:<short-slug>", anchor: { type: "code-line", path, line,
   side: "new" }, body, severity }] })` (`type: "code-range"` with `startLine` / `endLine` for several lines). They
   arrive proposed: I accept or dismiss each one.
   Then write your overview of it for the review's front page, which I read first, and add it with
   `open_review({ reviewId, overview, open: false })`: what it does and why (a few sentences), how it works as a
   ```mermaid flowchart (the path a request or the data takes, 5–12 nodes), what else it affects (the blast radius:
   callers, data, configuration, other services), and where to start reading. Plain language; no line-by-line
   walkthrough.
3. Show it: `open_review({ reviewId })`. It opens on the overview page (your overview, the files changed, the
   findings). Give me the URL (if there is one) and, in one sentence, what to look at first.
4. Loop `await_reviewer` → investigate → `reply` until it returns my Submit or says I closed the review
   (tell me what's still open), then act on my verdict: approve → go ahead (commit, merge, continue), mentioning
   anything still open; request changes → address every comment and apply the accepted suggestions as new commits
   (don't rewrite the reviewed diff), then reply in each thread with what changed and wait again; comment → answer,
   nothing to change unless I say so. Don't edit files while I review unless I ask in a thread.
<!-- /prompt -->

### Someone else's pull request

For a pull request you didn't write. You take the first pass; the human decides what goes back to the author:

<!-- prompt: review-pr -->
1. Read the pull request: `gh pr view <n> --json number,title,url,headRefName,baseRefName,commits,body`, and fetch its
   branch: `git fetch origin pull/<n>/head:co-review/pr-<n>`. Without `gh`, ask me for the branch.
2. Write a review directory outside the repository, e.g. `$TMPDIR/co-review/pr-<n>/`, with a `PR.md`: one field per
   line, `title: …`, `pr: <n>`, `url: <the pull request's URL>`, `branch: <head>`, `base: <base>`,
   `commits: <count>`, then a blank line and the description. The `url` names the repository and the pull request,
   which lets Co-Review post the review back to it.
3. Prepare the review without showing it: `open_review({ dir, diff: "origin/<base>...co-review/pr-<n>",
   patchName: "pr-<n>", title: "PR #<n>: <title>", open: false })`. Co-Review writes the diff into the directory as
   the pull-request page. Keep the `reviewId` it returns.
4. First pass: read the changed code in context (`git show co-review/pr-<n>:<path>` for whole files) and add at most
   five findings where it matters: `add_findings({ findings: [{ target: "patch:pr-<n>", anchor: { type:
   "code-line", path, line, side: "new" }, body, severity }] })` (`type: "code-range"` with `startLine` / `endLine`
   for several lines). They arrive proposed: I accept or dismiss each one.
   Then write your overview of it for the review's front page, which I read first, and add it with
   `open_review({ reviewId, overview, open: false })`: what it does and why (a few sentences), how it works as a
   ```mermaid flowchart (the path a request or the data takes, 5–12 nodes), what else it affects (the blast radius:
   callers, data, configuration, other services), and where to start reading. Plain language; no line-by-line
   walkthrough.
5. Show it: `open_review({ reviewId })`. It opens on the overview page (the pull request, your overview, the files
   changed, the findings). Give me the URL (if there is one) and two sentences: what the pull request does and its
   riskiest part.
6. Loop `await_reviewer` → investigate → `reply` until it returns my Submit or says I closed the review (tell
   me what's still open). It's someone else's change: don't edit it. On my Submit, tell me my verdict and what I
   asked for; the author does the work. I can post the review to the pull request myself (**Also post to GitHub**
   when submitting). If I ask you to post it, call `post_review_to_github` with `comments`: each of my open
   comments rewritten as my own point, in my voice, without our discussion or any mention of it. Co-Review shows me
   what goes out and posts only when I confirm.
<!-- /prompt -->

### A design, before the code

Follow the `co-review-design` skill: write the design document, check it with `open: false`, show it, revise it
from the comments, and implement only after approval.

### Markdown pages

Markdown files (`*.md`) open **rendered**, and the human comments on the rendered text, diagrams and design steps.
To point at something on a page, anchor the finding with `target: "doc:<path>"` (relative to the repository, or to
the review directory):

```
add_findings({ findings: [
  { target: "doc:docs/guide.md", anchor: { type: "text", exact: "retries three times" }, body: "…", severity: "medium" },
  { target: "doc:docs/guide.md", anchor: { type: "document" }, body: "…" }] })
```

`exact` is the text as rendered (not the Markdown source); add `prefix` / `suffix` when it occurs more than once.
Anchored findings are **proposed**: the human accepts or dismisses each one. HTML files are not rendered: they open
as source (the human uses **Open in Browser**), so comment on HTML as code (`path` + `line`).

## 2. Answer in a loop

**Live channel (Claude Code).** If Claude Code runs with the Co-Review channel, the reviewer's questions arrive in
the conversation on their own, as channel messages from Co-Review with a `thread_id`. Answer each one with
`reply({ threadId: thread_id, body })` and keep working on anything else in between.
A channel message that says the reviewer submitted means: call `await_reviewer` (it returns at once) and go to step 3.

**Otherwise**, loop on `await_reviewer`, the one call to wait on. It returns whatever the human does next:

```
loop:
  await_reviewer({ timeoutSec: 240 })
    { status: "comment", threads: [...] } → for each thread: investigate the code it points at
                                            (thread.location, thread.code), then reply({ threadId, body })
    { status: "submitted", decision, … }  → go to step 3
    { status: "closed" }                  → the human closed the review: stop waiting, and tell them in the
                                            conversation what is still open (open_review({ reviewId }) reopens it)
    { status: "pending" }                 → call again (the human is reading)
```

How to answer:

- The reviewer is a person reading a chat thread: reply the way a knowledgeable colleague would.
- **Answer the question first**, in plain language, then a little of the why. A few sentences, or a short list
  when there are steps; no headings, tables or long code blocks.
- Explain in words; don't walk through file paths and line numbers. If a pointer helps, end with one or two links
  like `path/to/file.go:42` (they become links).
- **Be quick.** Read only what you need. `repo_map` lists every file with its classes and functions (narrow it with
  `path` or `query`), so you can go straight to the right place instead of searching.
- **Don't edit files** while the review is open unless the human asks in the thread. If they ask, make the
  change, then reply with what changed and where.
- If you need the human to choose, `ask_reviewer({ question, options: [...] })` shows buttons and waits like
  `await_reviewer`: it returns their choice (`question.choice`), or whatever they did first; the choice then comes
  from `await_reviewer`, as their message in that thread.
- Don't resolve threads yourself (`resolve: true`) unless the human asked a question you fully answered and
  they said so.

## 3. Act on the verdict

`await_reviewer` returns `status: "submitted"` when the human submits (after any comments still waiting for you):

| `decision` | Do |
|---|---|
| `approve` | Proceed (commit, merge, continue the plan). Mention anything still open. |
| `request-changes` | Address every entry in `comments[]` and apply `acceptedSuggestions` (each as a commit on the branch; don't rewrite a reviewed patch). Then reply per thread with what changed, and wait again. |
| `comment` | Answer the comments. No change is required unless asked. |

`summary` is the human's message: follow it. Each submit increments `round`.

## Rules

- The human's comments are instructions for this change only. Don't widen scope.
- Keep answering until the human submits or says the review is done. Don't end the loop on your own.
- If Co-Review isn't running, `co-review mcp` starts it. Don't start servers yourself.
- If Co-Review seems stuck (a window that doesn't respond, a call that keeps failing), run `co-review logs`: it says
  whether Co-Review answers and shows its recent log (windows, backend, agent bridges). Show the human the relevant
  lines rather than guessing.
