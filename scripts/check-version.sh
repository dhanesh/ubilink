#!/usr/bin/env bash
# Fail when anything disagrees with the app version (desktop/Cargo.toml):
# the core and registry crates, a stray "version" in desktop/tauri.conf.json (Tauri
# must read Cargo), or a CHANGELOG.md with no section for it (release notes come
# from that section). Part of `make verify`.
#   check-version.sh [ROOT]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${1:-$(cd "$HERE/.." && pwd)}"
v="$(bash "$HERE/version.sh" "$ROOT")"
fail=0
for toml in src-tauri/Cargo.toml registry/Cargo.toml; do
  got="$(bash "$HERE/version.sh" "$ROOT" "$toml")"
  if [ "$got" != "$v" ]; then
    echo "check-version: $toml is $got, expected $v (from desktop/Cargo.toml)" >&2
    fail=1
  fi
done
if grep -q '"version"' "$ROOT/desktop/tauri.conf.json"; then
  echo "check-version: desktop/tauri.conf.json must not set \"version\" (Tauri reads desktop/Cargo.toml)" >&2
  fail=1
fi
if ! grep -qF "## [$v]" "$ROOT/CHANGELOG.md" 2>/dev/null; then
  echo "check-version: CHANGELOG.md has no '## [$v]' section" >&2
  fail=1
fi
exit "$fail"
