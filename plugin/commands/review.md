---
description: Review your change (its diff) with me in Co-Review as a pull-request page, and act on my verdict
argument-hint: "[base branch, e.g. main; default: the uncommitted changes]"
---
Review the change you made with me in Co-Review, following the `co-review` skill ("Your change"):

<!-- prompt: review-change -->
1. Prepare the review without showing it: `open_review({ diff: "HEAD", patchName: "<short-slug>", title: "<what the
   change does>", open: false })` for uncommitted work (new files included), or `diff: "<base>...HEAD"` for a branch
   (`<base>` is the branch it will merge into, usually `main`). Co-Review runs the diff itself and makes it a
   pull-request page. Keep the `reviewId` it returns. If it says the diff is empty, tell me and stop.
2. Add findings only for real risks and non-obvious decisions, at most five, on the changed lines:
   `add_findings({ findings: [{ target: "patch:<short-slug>", anchor: { type: "code-line", path, line, side: "new" },
   body, severity }] })` (`type: "code-range"` with `startLine` / `endLine` for several lines).
   Then write my overview of it for the review's front page and add it with `open_review({ reviewId, overview,
   open: false })`: what it does and why (a few sentences), how it works as a ```mermaid flowchart (the path a request
   or the data takes, 5–12 nodes), what else it affects (the blast radius: callers, data, configuration, other
   services), and where to start reading. Plain language; no line-by-line walkthrough.
3. Show it: `open_review({ reviewId })`. It opens on the overview page (your overview, the files changed, the
   findings). Give me the URL (if there is one) and, in one sentence, what to look at first.
4. Loop `await_reviewer` → investigate → `reply` until it returns my Submit or says I closed the review, then
   act on my verdict:
   approve → go ahead; request changes → address every comment and apply the accepted suggestions as new commits
   (don't rewrite the reviewed diff), then reply in each thread with what changed. Don't edit files while I review
   unless I ask in a thread.
<!-- /prompt -->

Base: ${ARGUMENTS:-none: review the uncommitted changes (git diff HEAD)}
