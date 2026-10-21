1. Read the pull request: `gh pr view <n> --json number,title,url,headRefName,baseRefName,commits,body`, and fetch its
   branch: `git fetch origin pull/<n>/head:co-review/pr-<n>`. Without `gh`, ask me for the branch.
2. Write a review directory outside the repository, e.g. `$TMPDIR/co-review/pr-<n>/`, with a `PR.md`: one field per
   line, `title: …`, `pr: <n>`, `url: <the pull request's URL>`, `branch: <head>`, `base: <base>`,
   `commits: <count>`, then a blank line and the description. The `url` names the repository and the pull request,
   which lets Co-Review post the review back to it.
3. Prepare the review without showing it: `{{open_review}}({ dir, diff: "origin/<base>...co-review/pr-<n>",
   patchName: "pr-<n>", title: "PR #<n>: <title>", open: false })`. Co-Review writes the diff into the directory as
   the pull-request page. Keep the `reviewId` it returns.
4. First pass: read the changed code in context (`git show co-review/pr-<n>:<path>` for whole files) and add at most
   five findings where it matters: `{{add_findings}}({ findings: [{ target: "patch:pr-<n>", anchor: { type:
   "code-line", path, side: "new" }, code, body, severity }] })`, where `code` is the line(s) the finding is about,
   copied exactly from the diff: Co-Review places the finding on them and refuses a quote that matches nothing (add
   `line` only to pick one of several matches). They arrive proposed: I accept or dismiss each one.
   {{> graphify}}
   {{> overview}}
5. Show it: `{{open_review}}({ reviewId })`. It opens on the overview page (the pull request, your overview, the files
   changed, the findings). Tell me it is open in Co-Review, with two sentences: what the pull request does and its
   riskiest part.
6. Loop `{{await_reviewer}}` → investigate → `{{reply}}` until it returns my Submit or says I closed the review (closing
   it means I'm done: tell me in one line what's still open). It's someone else's change: don't edit it. On my Submit, tell me my verdict and what I
   asked for; the author does the work. I can post the review to the pull request myself (**Also post to GitHub**
   when submitting): the Submit's `github.status` says whether I did (`posted`, or `posting` while Co-Review posts it).
   Then it is on GitHub: never say it isn't, and don't offer to post it. If I ask you to post it, call `{{post_review_to_github}}` with `comments`: each of my open
   comments rewritten as my own point, in my voice, without our discussion or any mention of it (a finding I kept and
   replied to is one point made of both: the finding with what I added). Co-Review shows me
   what goes out and posts only when I confirm.
