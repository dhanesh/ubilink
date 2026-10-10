#!/usr/bin/env bash
# Print a crate's [package] version. The app's version — desktop-gpui/Cargo.toml (the
# shipped GPUI app since Phase 2b) — is the single source of truth for releases (spec §5).
#   version.sh [ROOT] [TOML]   defaults: the repo root, desktop-gpui/Cargo.toml
set -euo pipefail
ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
TOML="${2:-desktop-gpui/Cargo.toml}"
v="$(awk '/^\[/ { section = $0 } section == "[package]" && /^version[[:space:]]*=/ { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }' "$ROOT/$TOML")"
[ -n "$v" ] || { echo "version.sh: no [package] version in $ROOT/$TOML" >&2; exit 1; }
printf '%s\n' "$v"
