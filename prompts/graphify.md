When the repository has [Graphify](https://graphify.net) graphs, ask them how the code connects before
   searching: `graphify query "<question>"`, `graphify path "A" "B"`, `graphify explain "X"`. A large repository may
   keep one graph per folder (e.g. `services/graphify-out/`; its AGENTS.md says which), and graphs are untracked, so a
   git worktree has none: pass `--graph <path>` to the right `graph.json`, the main checkout's in a worktree (the
   first line of `git worktree list`). If the folder it names has no `graph.json` (only dated snapshots), use the
   whole repository's, `graphify-out/graph.json`. `{{repo_map}}({ overview: true })` names the graphs it found. Only
   when there is none, say so and search instead.
