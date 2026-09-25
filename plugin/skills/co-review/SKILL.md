---
name: co-review
description: Hand your work to the human for review in Co-Review and stay in the review as their co-reviewer — open the review, add findings, answer their questions in the threads, and act on their verdict. Use when the user says "review this with me", "open a review", "co-review", "let me review your change", "walk me through the change", "I want to review before you continue", or when a change is ready and the human should look at it before it is committed or merged. Needs Co-Review's MCP tools (open_review, await_comment, reply, add_findings, await_review, get_review).
---

# Co-review a change

Co-Review is a review app where the human comments on code, designs and patches inline, and you answer in
the same threads. You are the co-reviewer: you did (or know) the work, so you explain and defend it, and you
change nothing until the human decides.

The MCP tools are registered as `co-review`. If they aren't
available, tell the user to run `claude mcp add -s user co-review -- co-review mcp` and stop.

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

## 3. Answer in a loop

```
loop:
  await_comment({ timeoutSec: 240 })     → { status: "comment", threads: [...] } or { status: "pending" }
  pending → call again (the human is reading)
  for each thread in threads:
    investigate the code it points at (thread.location, thread.code)
    reply({ threadId, body })
```

How to answer:

- **Answer the question first**, in one or two sentences, then the evidence.
- Reference code as `path/to/file.go:42` or `path:40-48`. They become links.
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
