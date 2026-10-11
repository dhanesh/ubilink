#!/usr/bin/env bash
# Tests for scripts/version.sh and scripts/check-version.sh (run by `make test-scripts`).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
pass=0
fail=0

# mk DIR GPUI_VER CORE_VER REGISTRY_VER APP_VER CHANGELOG_VER
# desktop-gpui/Cargo.toml is the source of truth.
mk() {
  mkdir -p "$1/desktop-gpui" "$1/core" "$1/registry" "$1/app"
  # A dependency `version = "…"` after [package], so version.sh must read [package].
  printf '[package]\nname = "g"\nversion = "%s"\n\n[dependencies]\ngpui-kit = { version = "=0.7.0" }\n' "$2" >"$1/desktop-gpui/Cargo.toml"
  printf '[package]\nname = "c"\nversion = "%s"\n' "$3" >"$1/core/Cargo.toml"
  printf '[package]\nname = "r"\nversion = "%s"\n' "$4" >"$1/registry/Cargo.toml"
  printf '[package]\nname = "a"\nversion = "%s"\n' "$5" >"$1/app/Cargo.toml"
  printf '# Changelog\n\n## [%s] - unreleased\n\n- x\n' "$6" >"$1/CHANGELOG.md"
}

# expect NAME WANT_EXIT DIR [STDERR_SUBSTRING]
# Exact exit codes (a missing script exits 127, so it can't pass a "want 1" case)
# and, for failures, the reason printed on stderr.
expect() {
  local err rc
  err="$(bash "$HERE/check-version.sh" "$3" 2>&1 >/dev/null)"
  rc=$?
  if [ "$rc" = "$2" ] && { [ -z "${4:-}" ] || printf '%s' "$err" | grep -qF -- "$4"; }; then
    pass=$((pass + 1))
  else
    echo "FAIL: $1 (exit $rc, want $2; stderr: $err)"
    fail=$((fail + 1))
  fi
}

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

mk "$T/ok" 0.1.0 0.1.0 0.1.0 0.1.0 0.1.0
expect "all agree" 0 "$T/ok"
mk "$T/core" 0.1.0 0.0.1 0.1.0 0.1.0 0.1.0
expect "core version differs" 1 "$T/core" "core/Cargo.toml is 0.0.1, expected 0.1.0 (from desktop-gpui/Cargo.toml)"
mk "$T/reg" 0.1.0 0.1.0 0.2.0 0.1.0 0.1.0
expect "registry version differs" 1 "$T/reg" "registry/Cargo.toml is 0.2.0, expected 0.1.0"
mk "$T/app" 0.1.0 0.1.0 0.1.0 0.3.0 0.1.0
expect "app version differs" 1 "$T/app" "app/Cargo.toml is 0.3.0, expected 0.1.0"
mk "$T/cl" 0.1.0 0.1.0 0.1.0 0.1.0 0.0.9
expect "changelog has no section for the version" 1 "$T/cl" "CHANGELOG.md has no '## [0.1.0]' section"
# A stray desktop/ (the deleted Tauri crate) is not consulted: its version cannot fail
# the check or become the app version.
mk "$T/stray" 0.2.0 0.2.0 0.2.0 0.2.0 0.2.0
mkdir -p "$T/stray/desktop"
printf '[package]\nname = "d"\nversion = "0.1.0"\n' >"$T/stray/desktop/Cargo.toml"
expect "a stray desktop/ crate is ignored" 0 "$T/stray"

# version.sh reads desktop-gpui's [package], not a dependency's `version = "=0.7.0"`,
# and not another crate's — the core is 0.0.1 in $T/core.
if [ "$(bash "$HERE/version.sh" "$T/ok")" = "0.1.0" ]; then pass=$((pass + 1)); else echo "FAIL: version.sh app version"; fail=$((fail + 1)); fi
if [ "$(bash "$HERE/version.sh" "$T/core")" = "0.1.0" ]; then pass=$((pass + 1)); else echo "FAIL: version.sh reads desktop-gpui, not core"; fail=$((fail + 1)); fi
if [ "$(bash "$HERE/version.sh" "$T/stray")" = "0.2.0" ]; then pass=$((pass + 1)); else echo "FAIL: version.sh reads desktop-gpui, not desktop"; fail=$((fail + 1)); fi
if [ "$(bash "$HERE/version.sh" "$T/core" core/Cargo.toml)" = "0.0.1" ]; then pass=$((pass + 1)); else echo "FAIL: version.sh other toml"; fail=$((fail + 1)); fi

echo "test-check-version: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
