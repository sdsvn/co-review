1. Prepare the review without showing it: `{{open_review}}({ diff: "HEAD", patchName: "<short-slug>", title: "<what the
   change does>", open: false })` for uncommitted work (new files included), or `diff: "<base>...HEAD"` for a branch
   (`<base>` is the branch it will merge into, usually `main`). Co-Review runs the diff itself and makes it a
   pull-request page. Keep the `reviewId` it returns. If it says the diff is empty, tell me and stop.
2. Add findings only for real risks and non-obvious decisions, at most five, on the changed lines:
   `{{add_findings}}({ findings: [{ target: "patch:<short-slug>", anchor: { type: "code-line", path, line, side: "new" },
   body, severity }] })` (`type: "code-range"` with `startLine` / `endLine` for several lines).
   {{> overview}}
3. Show it: `{{open_review}}({ reviewId })`. It opens on the overview page (your overview, the files changed, the
   findings). Give me the URL (if there is one) and, in one sentence, what to look at first.
4. Loop `{{await_reviewer}}` → investigate → `{{reply}}` until it returns my Submit or says I closed the review, then
   act on my verdict:
   approve → go ahead; request changes → address every comment and apply the accepted suggestions as new commits
   (don't rewrite the reviewed diff), then reply in each thread with what changed. Don't edit files while I review
   unless I ask in a thread.
