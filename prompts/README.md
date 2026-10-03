# Shared agent prompts

The instructions Co-Review gives agents live here, once. `make prompts` copies them into every place an agent reads
them: the Claude Code plugin (skills, commands, subagent), the Pi and Oh My Pi packages, the app's MCP and ACP text
(`extensions/review/src/node/prompts.gen.ts`), `llms.txt` and the docs. **Edit these files, then run `make prompts`.**
`make check-prompts` fails when a copy is out of date.

In a Markdown file, a copy sits between markers, and the generator rewrites what's in between:

    <!-- prompt: design pi -->
    …generated…
    <!-- /prompt -->

The marker names the fragment (`design.md`), optionally a harness (`mcp`, the default, or `pi`) and `fenced` (wrap
the copy in a ```text block). In a fragment, `{{open_review}}` and the other tool names become the harness's name
for that tool (the seven in `TOOLS` in `bin/gen-prompts.mjs`; `get_review` has the same name everywhere), and
`{{> answer-style}}` includes another fragment.

Conventions:

- The fragments speak as the reviewer to the agent: "I" and "me" are the human, "you" is the agent. Keep that voice.
- An include is pasted verbatim, so a fragment included inside a numbered step (`overview.md`) starts on the
  caller's line and indents its own continuation lines by three spaces.
- Wrap at about 115 characters, except `answer-style*.md`, which are single lines embedded as strings.
- The tool calls and result fields a fragment names (`format.warnings`, `status: "proposed"`, anchor types) are the
  real ones from `extensions/review/src/node/co-reviewer-mcp.ts`; check there before changing one.

| Fragment | Used by |
|---|---|
| `answer-style.md`, `answer-style-short.md` | every agent answering in a thread: ACP prompt, MCP instructions and tool results, the co-reviewer subagents |
| `co-reviewer.md` | the Pi and Oh My Pi co-reviewer agents |
| `design.md` | writing a design document: the design skill and commands, the `open_review` description, `llms.txt`, the docs |
| `audit.md` | the whole-repository audit: the commands and the `co-review` skill |
| `review-change.md` | reviewing the agent's change (its diff): the commands and the `co-review` skill |
| `review-pr.md` | reviewing someone else's pull request: the commands and the `co-review` skill |
| `pi-tools.md` | the Pi / Oh My Pi tool names, in the skills |
| `overview.md` | writing the review's overview (its front page), included by `audit.md`, `review-change.md`, `review-pr.md` |
