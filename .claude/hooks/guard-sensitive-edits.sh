#!/usr/bin/env bash
# PreToolUse guard (agent rails R5/R6): no silent edits to CI config, the
# Makefile gate, or Claude settings — wherever the checkout lives, including
# agent worktrees under .claude/worktrees/<name>/.
#
# Why a hook as well as the Edit(...) ask rules in settings.json: `/path` rules
# are anchored to the settings source and did not reliably cover background
# agents in worktree checkouts. A hook sees the absolute target path.
#
# Decision for a protected path:
#   - "ask" in modes where a person is asked (default, acceptEdits, plan);
#   - "deny" in modes where nobody is asked (auto, bypassPermissions, dontAsk).
#     In auto mode an "ask" is decided by the auto-mode classifier, not by you,
#     and a live test showed it silently approving a worktree agent's edit to
#     .github/CODEOWNERS. A deny holds in every mode; switch to default mode
#     (Shift+Tab) to make such an edit deliberately.
# Unprotected paths: no output (exit 0), so the normal rules apply.
set -uo pipefail

emit() { # $1 = ask|deny, $2 = reason
  if command -v jq >/dev/null 2>&1; then
    jq -n --arg d "$1" --arg r "$2" \
      '{hookSpecificOutput: {hookEventName: "PreToolUse", permissionDecision: $d, permissionDecisionReason: $r}}'
  else
    printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"%s","permissionDecisionReason":"guard-sensitive-edits: jq is missing, so the target path could not be checked."}}\n' "$1"
  fi
  exit 0
}

# Without jq nothing can be read: fail closed as far as we can (ask).
command -v jq >/dev/null 2>&1 || emit ask "jq missing"

input=$(cat)
path=$(jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' <<<"$input" 2>/dev/null) ||
  emit ask "guard-sensitive-edits: could not parse the tool input"
[ -n "$path" ] || exit 0
mode=$(jq -r '.permission_mode // empty' <<<"$input" 2>/dev/null) || mode=""

case "$path" in
  .github/* | */.github/* | Makefile | */Makefile | .claude/settings*.json | */.claude/settings*.json) ;;
  *) exit 0 ;;
esac

what="Protected path (CI config, the make gate, or Claude settings): $path."
case "$mode" in
  auto | bypassPermissions | dontAsk)
    emit deny "$what Refused in $mode mode, where no one is asked. Switch to default mode (Shift+Tab) and retry if this edit is intended."
    ;;
  *)
    emit ask "$what Confirm this edit."
    ;;
esac
