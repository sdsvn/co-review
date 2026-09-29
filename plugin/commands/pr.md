---
description: Review someone else's pull request with me in Co-Review, starting from your first-pass findings
argument-hint: "<pull request number or URL>"
---
Review this pull request with me in Co-Review, following the `co-review` skill ("Someone else's pull request"):
$ARGUMENTS

<!-- prompt: review-pr -->
1. Read the pull request: `gh pr view <n> --json number,title,url,headRefName,baseRefName,commits,body`, and fetch its
   branch: `git fetch origin pull/<n>/head:co-review/pr-<n>`. Without `gh`, ask me for the branch.
2. Write a review directory outside the repository, e.g. `$TMPDIR/co-review/pr-<n>/`, with a `PR.md`: `title: …`,
   `pr: <n>`, `url: <the pull request's URL>`, `branch: <head>`, `base: <base>`, `commits: <count>` (one per line), a
   blank line, then the description. The `url` lets Co-Review post the review back to the pull request.
3. Prepare the review without showing it: `open_review({ dir, diff: "origin/<base>...co-review/pr-<n>",
   patchName: "pr-<n>", title: "PR #<n>: <title>", open: false })`. Co-Review writes the diff into the directory as
   the pull-request page. Keep the `reviewId` it returns.
4. First pass: read the changed code in context (`git show co-review/pr-<n>:<path>` for whole files) and add at most
   five findings where it matters: `add_findings({ findings: [{ target: "patch:pr-<n>", anchor: { type:
   "code-line", path, line, side: "new" }, body, severity }] })`. They arrive proposed: I accept or dismiss each one.
5. Show it: `open_review({ reviewId })`. Give me the URL (if there is one) and two sentences: what the pull request
   does and its riskiest part.
6. Loop `await_reviewer` → investigate → `reply` until it returns my Submit. It's someone else's change: don't edit it.
   I can post the review to the pull request myself (**Also post to GitHub** when submitting). If I ask you to post
   it, call `post_review_to_github`: Co-Review shows me what goes out and posts only when I confirm.
<!-- /prompt -->
