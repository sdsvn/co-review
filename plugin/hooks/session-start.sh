#!/bin/sh
# Claude Code SessionStart hook: tells the session about open Co-Review reviews of this project.
# Reads the review data through the co-review command (never starts Co-Review); prints nothing when there is nothing
# to say, or when the command is missing or fails.
command -v co-review >/dev/null 2>&1 || exit 0
output=$(co-review status "${CLAUDE_PROJECT_DIR:-$PWD}" --claude-hook 2>/dev/null) || exit 0
[ -n "$output" ] && printf '%s\n' "$output"
exit 0
