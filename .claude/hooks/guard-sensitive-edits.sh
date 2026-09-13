#!/usr/bin/env bash
# PreToolUse guard (agent rails R5/R6): ask before any edit to CI config, the
# Makefile gate, or Claude settings — wherever the checkout lives, including
# agent worktrees under .claude/worktrees/<name>/.
#
# Why a hook as well as the Edit(...) ask rules in settings.json: `/path` rules
# are anchored to the settings source, and in practice they did not prompt for
# an edit made by a background agent inside a worktree checkout. A hook sees the
# absolute target path, so it does not depend on how rules are anchored.
#
# Input: the PreToolUse JSON on stdin. Output: a permissionDecision of "ask" for
# protected paths; nothing (exit 0) otherwise, so the normal rules apply.
set -uo pipefail

ask() {
  local reason="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -n --arg r "$reason" \
      '{hookSpecificOutput: {hookEventName: "PreToolUse", permissionDecision: "ask", permissionDecisionReason: $r}}'
  else
    printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"guard-sensitive-edits: jq is missing, so the target path could not be checked."}}'
  fi
  exit 0
}

# Without jq the path can't be read: fail closed (ask) rather than open.
command -v jq >/dev/null 2>&1 || ask "jq missing"

path=$(jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' 2>/dev/null) || ask "could not parse the tool input"
[ -n "$path" ] || exit 0

case "$path" in
  .github/* | */.github/* | Makefile | */Makefile | .claude/settings*.json | */.claude/settings*.json)
    ask "Protected path (CI config, the make gate, or Claude settings): $path. Confirm this edit."
    ;;
esac
exit 0
