#!/usr/bin/env bash
# Fail when anything disagrees with the app version (desktop-gpui/Cargo.toml): the
# core, registry and app crates, the Tauri crate and a stray "version" in its
# tauri.conf.json while desktop/ still exists, or a CHANGELOG.md with no section for it
# (release notes come from that section). Part of `make verify`.
#   check-version.sh [ROOT]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${1:-$(cd "$HERE/.." && pwd)}"
v="$(bash "$HERE/version.sh" "$ROOT")"
fail=0
tomls=(src-tauri/Cargo.toml registry/Cargo.toml app/Cargo.toml)
# desktop/ (the Tauri shell) is checked only while it is still in the tree.
if [ -f "$ROOT/desktop/Cargo.toml" ]; then tomls+=(desktop/Cargo.toml); fi
for toml in "${tomls[@]}"; do
  got="$(bash "$HERE/version.sh" "$ROOT" "$toml")"
  if [ "$got" != "$v" ]; then
    echo "check-version: $toml is $got, expected $v (from desktop-gpui/Cargo.toml)" >&2
    fail=1
  fi
done
if [ -f "$ROOT/desktop/tauri.conf.json" ] && grep -q '"version"' "$ROOT/desktop/tauri.conf.json"; then
  echo "check-version: desktop/tauri.conf.json must not set \"version\" (Tauri reads desktop/Cargo.toml)" >&2
  fail=1
fi
if ! grep -qF "## [$v]" "$ROOT/CHANGELOG.md" 2>/dev/null; then
  echo "check-version: CHANGELOG.md has no '## [$v]' section" >&2
  fail=1
fi
exit "$fail"
