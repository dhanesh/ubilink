#!/usr/bin/env bash
# Tests for scripts/changelog-section.sh (run by `make test-scripts`).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
pass=0
fail=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
cat >"$T/CHANGELOG.md" <<'MD'
# Changelog

## [0.2.0] - unreleased

- two

## [0.1.0] - 2026-09-14

### Added
- one

## [0.0.9] - old

- older
MD
printf '# Changelog\n\n## [0.3.0]\n\n' >"$T/empty.md"

expect() { # NAME WANT GOT
  if [ "$2" = "$3" ]; then pass=$((pass + 1)); else echo "FAIL: $1 — want [$2], got [$3]"; fail=$((fail + 1)); fi
}

got="$(bash "$HERE/changelog-section.sh" 0.1.0 "$T/CHANGELOG.md" | tr '\n' '|')"
expect "middle section only" "|### Added|- one||" "$got"
got="$(bash "$HERE/changelog-section.sh" 0.2.0 "$T/CHANGELOG.md" | tr '\n' '|')"
expect "first section" "|- two||" "$got"
# Exact exit codes: a missing script exits 127, so it can't pass these.
bash "$HERE/changelog-section.sh" 9.9.9 "$T/CHANGELOG.md" >/dev/null 2>&1
expect "missing version fails" 1 "$?"
bash "$HERE/changelog-section.sh" 0.3.0 "$T/empty.md" >/dev/null 2>&1
expect "empty section fails" 1 "$?"
bash "$HERE/changelog-section.sh" 0.1 "$T/CHANGELOG.md" >/dev/null 2>&1
expect "prefix version does not match 0.1.0" 1 "$?"

echo "test-changelog-section: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
