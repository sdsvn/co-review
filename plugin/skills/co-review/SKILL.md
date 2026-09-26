---
name: co-review
description: Hand your work to the human for review in Co-Review and stay in the review as their co-reviewer — open the review, add findings, answer their questions in the threads, and act on their verdict. Use when the user says "review this with me", "open a review", "co-review", "let me review your change", "walk me through the change", "I want to review before you continue", or when a change is ready and the human should look at it before it is committed or merged. Needs Co-Review's MCP tools (open_review, await_comment, reply, add_findings, await_review, get_review).
---

# Co-review a change

Co-Review is a review app where the human comments on code, designs and patches inline, and you answer in
the same threads. You are the co-reviewer: you did (or know) the work, so you explain and defend it, and you
change nothing until the human decides.

In Claude Code the tools come with the Co-Review plugin (`/plugin install co-review@co-review`) or
`claude mcp add -s user co-review -- co-review mcp`. If none are available, tell the user how to add them and stop.

<!-- prompt: pi-tools -->
> **In Pi or Oh My Pi** (the Co-Review package) the tools are named `co_review_start` (open_review; `dir` for a
> review directory), `co_review_wait` (await_comment), `co_review_reply`, `co_review_add_findings`, `co_review_ask`,
> `co_review_verdict` (await_review) and `co_review_map` (repo_map). In an interactive session, the reviewer's
> questions also arrive on their own as `[Co-Review]` messages.
<!-- /prompt -->

## 1. Open the review

- **Code in a repository**: `open_review({ root: <absolute repo path> })`. Pass `title` to start a new review
  instead of reusing the current one.
- **A design doc and/or patch**: write them to a review directory and call `open_review({ dir })`. See the
  `co-review-design` skill for design documents.

Give the user the returned `url` in one line. If the result has `note` instead (desktop app), say the review
is open in Co-Review. If `format.warnings` is non-empty, fix the document and call `open_review` again.

## 2. Seed findings (optional)

Before the human starts, point at what deserves attention in your own change:

```
add_findings({ findings: [{ path: "internal/orders/service.go", line: 31, endLine: 33,
  body: "…why this matters, what you chose and why…", severity: "high" }] })
```

Only real risks and non-obvious decisions, at most 3–5. Don't list everything you changed.

### Markdown pages

Markdown files (`*.md`) open **rendered** in Co-Review, and the human comments on the rendered text, diagrams and
design steps. To point at something on a page, anchor the finding to it with `target: "doc:<path>"` (relative to
the repository, or to the review directory for `open_review({ dir })`):

```
add_findings({ findings: [
  { target: "doc:docs/guide.md", anchor: { type: "text", exact: "retries three times" }, body: "…", severity: "medium" },
  { target: "doc:docs/guide.md", anchor: { type: "document" }, body: "…" }] })
```

`exact` is the text as rendered (not the Markdown source); add `prefix` / `suffix` when it occurs more than once.
Anchored findings are **proposed**: the human accepts or dismisses each one. Comments on pages come back with
`target: "doc:<path>"`, `kind` and `source` (the quoted text).

HTML pages are not rendered in Co-Review: they open as source, and **Open in Browser** shows them in the system
browser. Comment on HTML as code (`path` + `line`).

### Reviewing the whole repository (first-pass audit)

When the human wants the **entire repository** reviewed rather than a change, do a first pass for them, so they
start from a map of what matters instead of 5,000 files:

1. Call `repo_map({ overview: true })` for where to start and how the code clusters (it uses a Graphify graph when
   the repository has one), and `repo_map` for every file and what it defines. Split the repository into **areas**: 4–10 parts a person
   would review separately (e.g. `api`, `storage`, `auth`, `build`), usually top-level folders or packages.
2. For each area, read the code that carries the most risk (entry points, anything handling input, money, auth,
   concurrency or persistence), not every file.
3. Add findings with the area as the first label and `status: "proposed"`, so the human accepts or dismisses each:

   ```
   add_findings({ findings: [{ path: "internal/orders/service.go", line: 31,
     body: "…what's wrong, why it matters, what to do…", severity: "high",
     labels: ["orders"], status: "proposed" }] })
   ```

   A finding about a whole area goes on its folder (`path` without `line`); one about the repository on `path: "."`.
4. At most 3 findings per area, and only ones you'd defend in a review. Say plainly when an area looks fine.
5. Tell the human the areas and counts in one short message, then answer their questions as usual. The panel's
   **By area** view groups the findings by their first label.

## 3. Answer in a loop

**Live channel (Claude Code).** If Claude Code runs with the Co-Review channel, the reviewer's questions arrive in
the conversation on their own, as channel messages from Co-Review with a `thread_id`. Answer each one with
`reply({ threadId: thread_id, body })` and keep working on anything else in between; don't block on `await_comment`.
A channel message that says the reviewer submitted means: call `await_review` (it returns at once) and go to step 4.

**Otherwise**, loop:

```
loop:
  await_comment({ timeoutSec: 240 })     → { status: "comment", threads: [...] } or { status: "pending" }
  pending → call again (the human is reading)
  for each thread in threads:
    investigate the code it points at (thread.location, thread.code)
    reply({ threadId, body })
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
- If you need the human to choose, `ask_reviewer({ question, options: [...] })` shows buttons and blocks
  until they pick.
- Don't resolve threads yourself (`resolve: true`) unless the human asked a question you fully answered and
  they said so.

## 4. Act on the verdict

When the human submits, `await_comment` stops returning questions. Call
`await_review({ timeoutSec: 300 })` and loop until `status: "submitted"`:

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
