#!/usr/bin/env bash
# Build Ubilink.app — and, for the dist modes, a DMG — with Apple's own tools only
# (no tauri-cli: Tauri embeds the frontend at compile time, so the binary is
# self-contained). Design: docs/superpowers/specs/2026-09-14-v0.1-release-design.md §3.
#
#   bundle-macos.sh dev      host-arch app signed with a stable self-signed identity, so
#                            macOS keeps Accessibility/Automation grants across rebuilds
#                            (what `make bundle` runs)
#   bundle-macos.sh dry-run  universal app + DMG, hardened runtime, ad-hoc signed, named
#                            …-UNNOTARIZED.dmg (`make dist` without Apple credentials)
#   bundle-macos.sh release  universal app + DMG, Developer ID, notarized and stapled
#                            (`make dist` with all six APPLE_* variables — see
#                            docs/RUNBOOK-release.md)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:-dev}"
# shellcheck source=scripts/release-mode.sh
. "$ROOT/scripts/release-mode.sh"

APP_NAME="Ubilink"
EXE="ubilink"
BUNDLE_ID="org.dhanesh.ubilink"
VERSION="$(bash "$ROOT/scripts/version.sh")"
ENTITLEMENTS="$ROOT/desktop/Ubilink.entitlements"
SRC_ICON="$ROOT/desktop/icons/icon.png"
OUT="$ROOT/desktop/target/bundle"
APP="$OUT/$APP_NAME.app"
BIN=""
DMG=""
KEYCHAIN=""
WORK="$(mktemp -d)"

log() { echo "[bundle:$MODE] $*" >&2; }
die() { echo "[bundle:$MODE] error: $*" >&2; exit 1; }
cleanup() {
  if [ -n "$KEYCHAIN" ]; then security delete-keychain "$KEYCHAIN" 2>/dev/null || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

case "$MODE" in
  dev | dry-run) ;;
  release)
    n="$(credential_count)"
    [ "$n" -eq "${#APPLE_VARS[@]}" ] ||
      die "release mode needs all ${#APPLE_VARS[@]} APPLE_* variables ($n set) — see docs/RUNBOOK-release.md"
    ;;
  *) die "unknown mode '$MODE' (expected dev, dry-run or release)" ;;
esac

build_binary() {
  if [ "$MODE" = dev ]; then
    log "building the release binary for this Mac…"
    cargo build --release --manifest-path "$ROOT/desktop/Cargo.toml"
    BIN="$ROOT/desktop/target/release/$EXE"
  else
    local t
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
      log "building for ${t}…"
      cargo build --release --locked --target "$t" --manifest-path "$ROOT/desktop/Cargo.toml"
    done
    mkdir -p "$OUT"
    BIN="$WORK/$EXE-universal"
    lipo -create -output "$BIN" \
      "$ROOT/desktop/target/aarch64-apple-darwin/release/$EXE" \
      "$ROOT/desktop/target/x86_64-apple-darwin/release/$EXE"
  fi
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
  <key>NSHighResolutionCapable</key><true/>
  <!-- Shown on the Automation (Apple Events) prompt when Ubilink scripts another app. -->
  <key>NSAppleEventsUsageDescription</key>
  <string>Ubilink reads the link (URL or file path) of the app you're in, to create a link.</string>
</dict>
</plist>
PLIST
  plutil -lint "$APP/Contents/Info.plist" >/dev/null
}

# A stable self-signed identity for dev builds: macOS keys Accessibility/Automation
# grants to the signing identity, so ad-hoc dev builds would lose them on every
# rebuild. Prints the identity name (or "-" for ad-hoc if it can't be created).
ensure_dev_identity() {
  local id="Ubilink Self-Signed" tmp err=""
  # Look for an IDENTITY (certificate + private key), not just a certificate:
  # find-certificate succeeds on a cert whose private key never imported, which
  # still leaves codesign unable to sign with it — a false success that sends the
  # build down the signed path and then fails confusingly.
  # NOT `find-identity -v`: -v lists only identities macOS considers VALID, and a
  # self-signed certificate has no trust chain, so it is reported as
  # CSSMERR_TP_NOT_TRUSTED and omitted — even though codesign signs with it fine.
  # Using -v here silently recreated and re-imported the identity on every build,
  # stacking duplicate certificates in the login keychain and still signing ad-hoc.
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
    # A throwaway transport password, because Apple's `security` rejects
    # empty-password p12s. OpenSSL 3 additionally needs -legacy (its default PKCS12
    # MAC is rejected too), but LibreSSL — which IS /usr/bin/openssl on a stock Mac —
    # has no such flag and fails outright when given it. So try -legacy, then fall
    # back, rather than assuming a build: assuming OpenSSL 3 silently produced ad-hoc
    # dev builds on every machine without Homebrew's openssl first on PATH, and an
    # ad-hoc signature makes macOS discard the app's Accessibility/Automation grants
    # on every single rebuild.
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
    # Say WHY. This fallback used to discard every error, so a stock-Mac failure was
    # undiagnosable from the build log and looked like a deliberate choice.
    if [ -n "$err" ] && [ -s "$err" ]; then
      log "  cause: $(tail -3 "$err" | tr '\n' ' ')"
    fi
    echo "-"
  fi
}

# Import the Developer ID certificate into a temporary keychain that is deleted on exit.
import_developer_id() {
  local kc_pass p12="$WORK/cert.p12"
  KEYCHAIN="$WORK/ubilink-signing.keychain-db"
  kc_pass="$(openssl rand -hex 24)"
  security create-keychain -p "$kc_pass" "$KEYCHAIN"
  security set-keychain-settings -lut 3600 "$KEYCHAIN"
  security unlock-keychain -p "$kc_pass" "$KEYCHAIN"
  printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode >"$p12"
  security import "$p12" -k "$KEYCHAIN" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null
  rm -f "$p12"
  security set-key-partition-list -S apple-tool:,apple: -s -k "$kc_pass" "$KEYCHAIN" >/dev/null
  # Read the current per-user search list one path per line and rebuild it as
  # separate array entries (bash 3.2 — macOS's /bin/bash — has no mapfile/readarray,
  # so a manual read loop is the portable way). Quoting matters here: a keychain path
  # can contain spaces, and unlike everything else this script touches, the result of
  # `-s` is written into the user's *persistent* login search list — an unquoted
  # $(...) word-splits those paths and corrupts that list for every app and every
  # future login, not just this script's own temporary state.
  local existing=() line
  while IFS= read -r line; do
    line="${line#"${line%%[![:space:]]*}"}"  # strip the leading whitespace `security` prints
    line="${line#\"}"; line="${line%\"}"      # strip the surrounding quotes
    [ -n "$line" ] && existing+=("$line")
  done < <(security list-keychains -d user)
  # Search the temporary keychain first so codesign finds the identity. Guarded:
  # under bash 3.2 (macOS's /bin/bash) with `set -u`, expanding an EMPTY array as
  # "${existing[@]}" is itself an unbound-variable error (fixed only in bash
  # 4.4+), so branch instead of relying on that expansion when the list is empty.
  if [ "${#existing[@]}" -gt 0 ]; then
    security list-keychains -d user -s "$KEYCHAIN" "${existing[@]}"
  else
    security list-keychains -d user -s "$KEYCHAIN"
  fi
}

sign_app() { # IDENTITY
  local args=(--force --options runtime --entitlements "$ENTITLEMENTS" --sign "$1")
  if [ "$MODE" = release ]; then args+=(--timestamp); fi
  codesign "${args[@]}" "$APP"
}

notarize() { # FILE (a .zip of the app, or the .dmg)
  local key="$WORK/AuthKey_${APPLE_API_KEY_ID}.p8" result status id
  result="$WORK/notary-$(basename "$1").json"
  [ -f "$key" ] || printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode >"$key"
  log "notarizing $(basename "$1") (waits for Apple)…"
  xcrun notarytool submit "$1" --key "$key" --key-id "$APPLE_API_KEY_ID" \
    --issuer "$APPLE_API_ISSUER" --wait --timeout 30m --output-format json >"$result" || true
  status="$(plutil -extract status raw -o - "$result" 2>/dev/null || echo "unknown")"
  if [ "$status" != "Accepted" ]; then
    id="$(plutil -extract id raw -o - "$result" 2>/dev/null || echo "unknown")"
    xcrun notarytool log "$id" --key "$key" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER" >&2 || true
    die "notarization of $(basename "$1") returned '$status' (submission $id)"
  fi
}

make_dmg() {
  local stage="$WORK/dmg"
  if [ "$MODE" = release ]; then
    DMG="$OUT/$APP_NAME-$VERSION-universal.dmg"
  else
    DMG="$OUT/$APP_NAME-$VERSION-universal-UNNOTARIZED.dmg"
  fi
  log "creating $(basename "$DMG")…"
  mkdir -p "$stage"
  cp -R "$APP" "$stage/"
  ln -s /Applications "$stage/Applications"
  rm -f "$DMG"
  # `hdiutil create -srcfolder ... -format UDZO` prints a deprecation warning
  # ("please use 'diskutil image create from/blank --volumeName --format ...'
  # instead") on recent macOS. Investigated switching to that: `diskutil image
  # create from --format UDZO --volumeName ...` does work, but only on macOS 26
  # (Tahoe) — it's undocumented in the installed man page (only in --help) and
  # absent on macOS 15 and earlier. This project's dry-run path (`make dist`)
  # needs to keep working for contributors on Sonoma/Sequoia, and a runtime
  # version check with two code paths is more complexity than a release right
  # before an rc warrants. So: kept `hdiutil create`, warning accepted for now.
  hdiutil create -volname "$APP_NAME" -srcfolder "$stage" -ov -format UDZO "$DMG" >/dev/null
  if [ "$MODE" = release ]; then
    codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG"
  else
    codesign --force --sign - "$DMG"
  fi
}

verify_outputs() {
  log "verifying…"
  local exe="$APP/Contents/MacOS/$EXE" archs ents keys nkeys entval plist="$APP/Contents/Info.plist"
  if [ "$MODE" != dev ]; then
    archs="$(lipo -archs "$exe")"
    case " $archs " in *" arm64 "*) ;; *) die "no arm64 slice (have: $archs)" ;; esac
    case " $archs " in *" x86_64 "*) ;; *) die "no x86_64 slice (have: $archs)" ;; esac
  fi
  codesign --verify --deep --strict --verbose=2 "$APP"
  # `|| die` (rather than letting `set -e` abort silently) so a codesign failure here
  # is reported by name, distinct from the entitlements-content checks below.
  ents="$(codesign -d --entitlements - --xml "$APP" 2>/dev/null)" ||
    die "codesign failed to read entitlements from $APP"
  keys="$(printf '%s' "$ents" | grep -o '<key>[^<]*</key>' || true)"
  nkeys="$(printf '%s' "$ents" | grep -c '<key>' || true)"
  [ "$nkeys" = 1 ] && [ "$keys" = "<key>com.apple.security.automation.apple-events</key>" ] ||
    die "entitlements must be exactly the Apple Events entitlement, got: ${keys:-none}"
  # Check the VALUE too, not just the key's presence — a stray <false/> would
  # otherwise pass. `plutil -extract … raw` reads the XML entitlements straight off
  # stdin; plutil keypaths treat "." as a path separator, so the key's own literal
  # dots must be escaped or the lookup silently fails to find it.
  entval="$(printf '%s' "$ents" | plutil -extract 'com\.apple\.security\.automation\.apple-events' raw -o - - 2>/dev/null || true)"
  [ "$entval" = true ] ||
    die "com.apple.security.automation.apple-events entitlement must be true, got: ${entval:-none}"
  [ "$(plutil -extract CFBundleShortVersionString raw -o - "$plist")" = "$VERSION" ] ||
    die "Info.plist version is not $VERSION"
  plutil -extract NSAppleEventsUsageDescription raw -o - "$plist" >/dev/null ||
    die "Info.plist has no NSAppleEventsUsageDescription"
  if [ -n "$DMG" ]; then hdiutil verify "$DMG" >/dev/null; fi
  if [ "$MODE" = release ]; then
    spctl -a -t exec -vv "$APP"
    xcrun stapler validate "$APP"
    xcrun stapler validate "$DMG"
  fi
}

build_binary
assemble_app
case "$MODE" in
  dev) sign_app "$(ensure_dev_identity)" ;;
  dry-run) sign_app - ;;
  release)
    import_developer_id
    sign_app "$APPLE_SIGNING_IDENTITY"
    ditto -c -k --keepParent "$APP" "$WORK/$APP_NAME.zip"
    notarize "$WORK/$APP_NAME.zip"
    xcrun stapler staple "$APP"
    ;;
esac
if [ "$MODE" != dev ]; then make_dmg; fi
if [ "$MODE" = release ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi
verify_outputs
log "done: $APP${DMG:+ and $DMG}"
if [ "$MODE" = dev ]; then
  log "grant Automation to THIS app once (or use the in-app onboarding)."
fi
