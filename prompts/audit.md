1. Prepare the review without showing it: `{{open_review}}({ title: "Repository review", open: false })`. Keep the
   `reviewId` it returns. Don't give me a URL yet.
2. Call `{{repo_map}}({ overview: true })` and `{{repo_map}}`. With [Graphify](https://graphify.net) installed, the
   overview is built from its code graph (Co-Review builds it: calls, imports, clusters, the most connected code), and
   `graphify-out/GRAPH_REPORT.md` lists import cycles and surprising connections between distant parts of the code:
   good leads. Split the repository into 4–10 areas and read the riskiest code in each (entry points, input handling,
   money, auth, concurrency, persistence).
3. Add findings with `{{add_findings}}`: the area as the first label, `status: "proposed"`, at most three per area,
   each with what's wrong, why it matters and what to do. Say so when an area looks fine.
4. Show it: `{{open_review}}({ reviewId })`. It opens on the findings page: every finding, grouped by area, as one
   page I can read and comment on. Give me the URL (if there is one) and the areas with their finding counts, in one
   short message.
5. Then loop `{{await_comment}}` → investigate → `{{reply}}` until I submit, and act on my verdict (`{{await_review}}`).
   Don't edit files while I review unless I ask in a thread.
