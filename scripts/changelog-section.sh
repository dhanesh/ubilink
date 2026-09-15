#!/usr/bin/env bash
# Print the CHANGELOG.md section for one version — the body between its
# "## [VERSION]" heading and the next "## [" heading. Used as release notes.
#   changelog-section.sh VERSION [CHANGELOG]
set -euo pipefail
v="${1:?usage: changelog-section.sh VERSION [CHANGELOG]}"
f="${2:-$(cd "$(dirname "$0")/.." && pwd)/CHANGELOG.md}"
out="$(awk -v head="## [$v]" 'index($0, head) == 1 { on = 1; next } on && /^## \[/ { exit } on { print }' "$f"; printf 'X')"
out="${out%X}"
if [ -z "$(printf '%s' "$out" | tr -d '[:space:]')" ]; then
  echo "changelog-section: no notes for $v in $f" >&2
  exit 1
fi
printf '%s' "$out"
