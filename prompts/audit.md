1. Call `{{open_review}}` for this repository with a `title` such as "Repository review" and `open: false`: this
   prepares the review without showing it, so it opens on your findings. Don't give me a URL yet.
2. Call `{{repo_map}}`, split the repository into 4–10 areas, and read the riskiest code in each.
3. Add findings with `{{add_findings}}`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do.
4. Call `{{open_review}}` again without a `title` (the same review) to show it, then give me the URL (if there is
   one) and the areas and how many findings each has, in one short message.
5. Then loop `{{await_comment}}` → investigate → `{{reply}}` until I submit, and act on my verdict (`{{await_review}}`).
   Don't edit files while I review unless I ask in a thread.
