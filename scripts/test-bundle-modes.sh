#!/usr/bin/env bash
# Tests for scripts/release-mode.sh (how a build picks its signing mode, the
# tag/version check) and for the parts of scripts/bundle-macos.sh that run before any
# build: mode/credential refusal, and that it packages the GPUI app (run by
# `make test-scripts`). Never runs dev mode — that imports a signing identity.
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

# bundle_refusal MODE N → "<exit> <reason found: yes|no>"; MODE must fail before building.
bundle_refusal() {
  local mode="$1" n="$2" want="$3" i=0 v envs=() err rc
  for v in "${ALL[@]}"; do
    if [ "$i" -lt "$n" ]; then envs+=("$v=x"); else envs+=("$v="); fi
    i=$((i + 1))
  done
  err="$(env "${envs[@]}" bash "$HERE/bundle-macos.sh" "$mode" 2>&1 >/dev/null)"
  rc=$?
  if printf '%s' "$err" | grep -qF -- "$want"; then echo "$rc yes"; else echo "$rc no"; fi
}

expect "bundle: unknown mode refused"        "1 yes" "$(bundle_refusal bogus 0 "unknown mode 'bogus'")"
expect "bundle: release without credentials" "1 yes" "$(bundle_refusal release 0 "needs all 6 APPLE_* variables (0 set)")"
expect "bundle: release, partial credentials" "1 yes" "$(bundle_refusal release 4 "needs all 6 APPLE_* variables (4 set)")"

# Static: the shipped bundle is the GPUI app, built without the automation feature, as a
# menu-bar app, from assets under desktop-gpui/packaging/ — and bundle-next.sh is gone
# (folded into bundle-macos.sh's dev mode).
B="$HERE/bundle-macos.sh"
has() { grep -qF -- "$1" "$B" && echo yes || echo no; }
expect "bundle: builds desktop-gpui"           yes "$(has 'MANIFEST="$ROOT/desktop-gpui/Cargo.toml"')"
expect "bundle: no desktop/ (Tauri) paths"     no  "$(grep -qE '\$ROOT/desktop/' "$B" && echo yes || echo no)"
expect "bundle: never enables a cargo feature" no  "$(grep -vE '^[[:space:]]*#' "$B" | grep -qE -- '--features|--all-features' && echo yes || echo no)"
expect "bundle: binary path from cargo JSON"   yes "$(has '--message-format=json')"
expect "bundle: LSUIElement in Info.plist"     yes "$(has '<key>LSUIElement</key><true/>')"
expect "bundle: verifies LSUIElement"          yes "$(has 'plutil -extract LSUIElement')"
expect "bundle: entitlements file exists"      yes "$([ -f "$HERE/../desktop-gpui/packaging/Ubilink.entitlements" ] && echo yes || echo no)"
expect "bundle: icon exists"                   yes "$([ -f "$HERE/../desktop-gpui/packaging/icon.png" ] && echo yes || echo no)"
expect "bundle-next.sh folded into bundle-macos.sh" no "$([ -e "$HERE/bundle-next.sh" ] && echo yes || echo no)"

echo "test-bundle-modes: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
