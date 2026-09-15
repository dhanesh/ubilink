#!/usr/bin/env bash
# Tests for scripts/release-mode.sh: how a build picks its signing mode, and the
# tag/version check (run by `make test-scripts`).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
pass=0
fail=0
ALL=(APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_API_KEY_ID APPLE_API_ISSUER APPLE_API_KEY_P8_BASE64)

# run_mode N REF → "<exit> <stdout>" with the first N credentials set, the rest empty.
run_mode() {
  local n="$1" ref="$2" i=0 v envs=() out rc
  for v in "${ALL[@]}"; do
    if [ "$i" -lt "$n" ]; then envs+=("$v=x"); else envs+=("$v="); fi
    i=$((i + 1))
  done
  out="$(env "${envs[@]}" bash "$HERE/release-mode.sh" "$ref" 2>/dev/null)"
  rc=$?
  echo "$rc $out"
}

check_tag() {
  bash "$HERE/release-mode.sh" --check-tag "$1" "$2" >/dev/null 2>&1
  echo $?
}

expect() { # NAME WANT GOT
  if [ "$2" = "$3" ]; then pass=$((pass + 1)); else echo "FAIL: $1 — want '$2', got '$3'"; fail=$((fail + 1)); fi
}

expect "no credentials, local build"       "0 dry-run" "$(run_mode 0 '')"
expect "no credentials, manual run"        "0 dry-run" "$(run_mode 0 refs/heads/main)"
expect "no credentials, rc tag"            "0 dry-run" "$(run_mode 0 refs/tags/v0.1.0-rc.1)"
expect "no credentials, final tag refused" "1 "        "$(run_mode 0 refs/tags/v0.1.0)"
expect "all credentials, final tag"        "0 release" "$(run_mode 6 refs/tags/v0.1.0)"
expect "all credentials, rc tag"           "0 release" "$(run_mode 6 refs/tags/v0.1.0-rc.2)"
expect "partial credentials refused"       "1 "        "$(run_mode 3 refs/tags/v0.1.0-rc.1)"
expect "one credential refused"            "1 "        "$(run_mode 1 '')"

expect "tag equals version"      0 "$(check_tag refs/tags/v0.1.0 0.1.0)"
expect "rc tag equals version"   0 "$(check_tag refs/tags/v0.1.0-rc.3 0.1.0)"
expect "tag differs"             1 "$(check_tag refs/tags/v0.2.0 0.1.0)"
expect "non-rc suffix refused"   1 "$(check_tag refs/tags/v0.1.0-beta 0.1.0)"
expect "dots are literal"        1 "$(check_tag refs/tags/v0x1x0 0.1.0)"
expect "branch ref refused"      1 "$(check_tag refs/heads/main 0.1.0)"

echo "test-bundle-modes: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
