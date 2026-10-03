#!/usr/bin/env bash
# Build ubilink-next.app — the GPUI shell (desktop-gpui/) — with Apple's own tools only,
# modelled on scripts/bundle-macos.sh. It ships BESIDE the Tauri Ubilink.app until
# Phase 2 (spec D4), so it has its own bundle id (org.ubilink.next) and executable
# (ubilink-next): macOS keys permission grants to the bundle id, and the two apps must
# never share them. Spec: docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md.
#
#   bundle-next.sh dev   host-arch app signed with the same stable self-signed identity
#                        as bundle-macos.sh, so macOS keeps Accessibility/Automation
#                        grants across rebuilds
#
# Only dev exists in Phase 0. The universal, notarized dry-run/release builds arrive
# with the Phase 2 cut-over, when this app replaces Ubilink.app.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:-dev}"

APP_NAME="ubilink-next"
EXE="ubilink-next"
BUNDLE_ID="org.ubilink.next"
# One version for the whole product: desktop/Cargo.toml stays the source of truth
# (scripts/version.sh, spec §5) until the Phase 2 cut-over.
VERSION="$(bash "$ROOT/scripts/version.sh")"
# Same entitlement (Apple Events only) and icon as the Tauri app — the capture flow
# scripts other apps exactly as it does today.
ENTITLEMENTS="$ROOT/desktop/Ubilink.entitlements"
SRC_ICON="$ROOT/desktop/icons/icon.png"
MANIFEST="$ROOT/desktop-gpui/Cargo.toml"
OUT="$ROOT/desktop-gpui/target/bundle"
APP="$OUT/$APP_NAME.app"
BIN="$ROOT/desktop-gpui/target/release/$EXE"
WORK="$(mktemp -d)"

log() { echo "[bundle-next:$MODE] $*" >&2; }
die() { echo "[bundle-next:$MODE] error: $*" >&2; exit 1; }
trap 'rm -rf "$WORK"' EXIT

case "$MODE" in
  dev) ;;
  dry-run | release)
    die "mode '$MODE' is not available yet — universal, notarized ubilink-next builds arrive in Phase 2 (use 'dev')"
    ;;
  *) die "unknown mode '$MODE' (expected dev; dry-run and release arrive in Phase 2)" ;;
esac

build_binary() {
  log "building the release binary for this Mac…"
  # --locked: desktop-gpui/Cargo.lock is committed, and the bundle must be built from
  # exactly that pinned GPUI stack, never a silently re-resolved one.
  cargo build --release --locked --manifest-path "$MANIFEST"
  [ -x "$BIN" ] || die "binary not found at $BIN"
}

assemble_app() {
  log "assembling $APP_NAME.app ${VERSION}…"
  [ -f "$SRC_ICON" ] || die "icon source missing: $SRC_ICON"
  rm -rf "$APP"
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
  cp "$BIN" "$APP/Contents/MacOS/$EXE"

  local iconset="$WORK/$APP_NAME.iconset" spec px name
  mkdir -p "$iconset"
  for spec in "16:16x16" "32:16x16@2x" "32:32x32" "64:32x32@2x" \
    "128:128x128" "256:128x128@2x" "256:256x256" "512:256x256@2x" \
    "512:512x512" "1024:512x512@2x"; do
    px="${spec%%:*}"
    name="${spec##*:}"
    sips -z "$px" "$px" "$SRC_ICON" --out "$iconset/icon_${name}.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$APP/Contents/Resources/icon.icns"

  # LSUIElement: a menu-bar utility — no Dock icon, no app menu — from the very first
  # frame of launch (main.rs re-asserts the Accessory policy after GPUI resets it).
  cat >"$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$EXE</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Shown on the Automation (Apple Events) prompt when ubilink scripts another app. -->
  <key>NSAppleEventsUsageDescription</key>
  <string>Ubilink reads the link (URL or file path) of the app you're in, to create a link.</string>
</dict>
</plist>
PLIST
  plutil -lint "$APP/Contents/Info.plist" >/dev/null
}

# The stable dev identity, copied from bundle-macos.sh's ensure_dev_identity (that
# script is not sourceable — it runs a build on load). Same identity NAME on purpose:
# both apps sign with the one certificate already in the login keychain, so no second
# self-signed cert is created. See bundle-macos.sh for why each step is shaped this way
# (find-identity without -v, the throwaway p12 password, the -legacy fallback).
ensure_dev_identity() {
  local id="Ubilink Self-Signed" tmp err=""
  if ! security find-identity -p codesigning 2>/dev/null | grep -Fq "$id"; then
    log "creating a one-time self-signed code-signing identity ($id)…"
    tmp="$WORK/dev-identity"
    mkdir -p "$tmp"
    err="$tmp/openssl.log"
    cat >"$tmp/req.cnf" <<CNF
[ req ]
distinguished_name = dn
prompt = no
x509_extensions = v3
[ dn ]
CN = $id
[ v3 ]
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
basicConstraints = critical, CA:false
CNF
    openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
      -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -config "$tmp/req.cnf" >>"$err" 2>&1
    openssl pkcs12 -export -legacy -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
      -name "$id" -out "$tmp/id.p12" -passout pass:ubilink-bundle >>"$err" 2>&1 ||
      openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
        -name "$id" -out "$tmp/id.p12" -passout pass:ubilink-bundle >>"$err" 2>&1
    security import "$tmp/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
      -P "ubilink-bundle" -T /usr/bin/codesign >>"$err" 2>&1
  fi
  if security find-identity -p codesigning 2>/dev/null | grep -Fq "$id"; then
    echo "$id"
  else
    log "WARNING: self-signed identity unavailable; signing ad-hoc (grants reset on rebuild)"
    if [ -n "$err" ] && [ -s "$err" ]; then
      log "  cause: $(tail -3 "$err" | tr '\n' ' ')"
    fi
    echo "-"
  fi
}

sign_app() { # IDENTITY
  codesign --force --options runtime --entitlements "$ENTITLEMENTS" --sign "$1" "$APP"
}

verify_outputs() {
  log "verifying…"
  local plist="$APP/Contents/Info.plist"
  codesign --verify --deep --strict --verbose=2 "$APP"
  [ "$(plutil -extract CFBundleIdentifier raw -o - "$plist")" = "$BUNDLE_ID" ] ||
    die "Info.plist bundle id is not $BUNDLE_ID"
  [ "$(plutil -extract CFBundleShortVersionString raw -o - "$plist")" = "$VERSION" ] ||
    die "Info.plist version is not $VERSION"
  [ "$(plutil -extract LSUIElement raw -o - "$plist")" = true ] ||
    die "Info.plist LSUIElement is not true"
}

build_binary
assemble_app
sign_app "$(ensure_dev_identity)"
verify_outputs
log "done: $APP ($(stat -f %z "$APP/Contents/MacOS/$EXE") bytes, stripped)"
