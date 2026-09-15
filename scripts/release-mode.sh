#!/usr/bin/env bash
# How a Ubilink build is signed (spec §3 "Safety rules"). Used by `make dist`,
# scripts/bundle-macos.sh and .github/workflows/release.yml.
#   release-mode.sh [GIT_REF]                      print "release" or "dry-run";
#                                                  exit 1 (reason on stderr) = do not build
#   release-mode.sh --check-tag GIT_REF VERSION    exit 0 iff GIT_REF is vVERSION or vVERSION-rc.N
# It can also be sourced for APPLE_VARS / credential_count / resolve_mode / tag_matches_version.

APPLE_VARS=(APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_API_KEY_ID APPLE_API_ISSUER APPLE_API_KEY_P8_BASE64)

credential_count() {
  local n=0 v
  for v in "${APPLE_VARS[@]}"; do
    if [ -n "${!v:-}" ]; then n=$((n + 1)); fi
  done
  echo "$n"
}

# resolve_mode GIT_REF (e.g. refs/tags/v0.1.0, refs/tags/v0.1.0-rc.1, refs/heads/main, "")
resolve_mode() {
  local ref="${1:-}" n total="${#APPLE_VARS[@]}"
  n="$(credential_count)"
  if [ "$n" -ne 0 ] && [ "$n" -ne "$total" ]; then
    echo "release-mode: partial Apple credentials ($n of $total set); set all of ${APPLE_VARS[*]} or none" >&2
    return 1
  fi
  if [ "$n" -eq "$total" ]; then
    echo release
    return 0
  fi
  case "$ref" in
    refs/tags/v*-*) echo dry-run ;; # pre-release tag: an un-notarized build is allowed
    refs/tags/v*)
      echo "release-mode: $ref is a final release tag but no Apple credentials are set; refusing to build an un-notarized final release" >&2
      return 1
      ;;
    *) echo dry-run ;; # manual run or local build
  esac
}

# tag_matches_version GIT_REF VERSION
tag_matches_version() {
  local ref="$1" ver="$2" tag
  case "$ref" in
    refs/tags/v*) tag="${ref#refs/tags/v}" ;;
    *)
      echo "release-mode: $ref is not a version tag" >&2
      return 1
      ;;
  esac
  if [ "$tag" = "$ver" ]; then return 0; fi
  case "$tag" in
    "$ver"-rc.*)
      local n="${tag#"$ver"-rc.}"
      case "$n" in '' | *[!0-9]*) ;; *) return 0 ;; esac
      ;;
  esac
  echo "release-mode: tag v$tag does not match version $ver (expected v$ver or v$ver-rc.N)" >&2
  return 1
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -uo pipefail
  if [ "${1:-}" = "--check-tag" ]; then
    tag_matches_version "${2:?git ref}" "${3:?version}"
  else
    resolve_mode "${1:-}"
  fi
fi
