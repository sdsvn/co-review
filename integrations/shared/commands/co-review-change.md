---
description: Review your change (its diff) with me in Co-Review as a pull-request page, and act on my verdict
argument-hint: "[base branch, e.g. main; default: the uncommitted changes]"
---
Review the change you made with me in Co-Review, following the `co-review` skill ("Your change"). Use the
Co-Review tools from the co-review extension.

<!-- prompt: review-change pi -->
1. Prepare the review without showing it: `co_review_start({ diff: "HEAD", patchName: "<short-slug>", title: "<what the
   change does>", open: false })` for uncommitted work (new files included), or `diff: "<base>...HEAD"` for a branch
   (`<base>` is the branch it will merge into, usually `main`). Co-Review runs the diff itself and makes it a
   pull-request page. Keep the `reviewId` it returns. If it says the diff is empty, tell me and stop.
2. Add findings only for real risks and non-obvious decisions, at most five, on the changed lines:
   `co_review_add_findings({ findings: [{ target: "patch:<short-slug>", anchor: { type: "code-line", path, line, side: "new" },
   body, severity }] })` (`type: "code-range"` with `startLine` / `endLine` for several lines).
3. Show it: `co_review_start({ reviewId })`. Give me the URL (if there is one) and, in one sentence, what to look at first.
4. Loop `co_review_wait` → investigate → `co_review_reply` until it returns my Submit or says I closed the review, then act on my verdict:
   approve → go ahead; request changes → address every comment and apply the accepted suggestions as new commits
   (don't rewrite the reviewed diff), then reply in each thread with what changed. Don't edit files while I review
   unless I ask in a thread.
<!-- /prompt -->

Base: $ARGUMENTS (none: review the uncommitted changes, `git diff HEAD`)
