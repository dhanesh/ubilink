#!/usr/bin/env bash
# Fail when anything disagrees with the app version (desktop-gpui/Cargo.toml): the
# core, registry and app crates, or a CHANGELOG.md with no section for it (release
# notes come from that section). Part of `make verify`.
#   check-version.sh [ROOT]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${1:-$(cd "$HERE/.." && pwd)}"
v="$(bash "$HERE/version.sh" "$ROOT")"
fail=0
for toml in src-tauri/Cargo.toml registry/Cargo.toml app/Cargo.toml; do
  got="$(bash "$HERE/version.sh" "$ROOT" "$toml")"
  if [ "$got" != "$v" ]; then
    echo "check-version: $toml is $got, expected $v (from desktop-gpui/Cargo.toml)" >&2
    fail=1
  fi
done
if ! grep -qF "## [$v]" "$ROOT/CHANGELOG.md" 2>/dev/null; then
  echo "check-version: CHANGELOG.md has no '## [$v]' section" >&2
  fail=1
fi
exit "$fail"
