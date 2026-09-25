#!/bin/sh
# Claude Code SessionStart hook: tells the session about open Co-Review reviews of this project.
# Asks a running Co-Review only (never starts it); prints nothing when there is nothing to say.
registry="${CO_REVIEW_HOME:-$HOME/.co-review}/server.json"
[ -f "$registry" ] || exit 0
url=$(sed -n 's/.*"url": *"\([^"]*\)".*/\1/p' "$registry")
[ -n "$url" ] || exit 0
curl -sf --max-time 2 -G "$url/api/m/status" --data-urlencode "root=${CLAUDE_PROJECT_DIR:-$PWD}" --data "format=claude-hook" 2>/dev/null
exit 0
