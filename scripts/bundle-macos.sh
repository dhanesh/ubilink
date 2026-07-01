#!/usr/bin/env bash
# Build a macOS .app bundle for ubilink WITHOUT tauri-cli — just the release binary
# (Tauri embeds the frontend at compile time, so it's self-contained) wrapped in a
# standard bundle, with an .icns icon and an ad-hoc code signature.
#
# Why a bundle: macOS TCC (Accessibility / Automation grants) keys on a stable app
# at a stable path with a stable identity. Granting a bare `cargo run` binary is
# finicky and re-prompts on rebuild; a signed .app at a fixed path makes the grant
# stick. Re-running this rebuilds in place, preserving the grant.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="ubilink"
BUNDLE_ID="org.dhanesh.ubilink"
VERSION="0.0.1"
BIN="$ROOT/desktop/target/release/$APP_NAME"
OUT="$ROOT/desktop/target/release/bundle"
APP="$OUT/$APP_NAME.app"
SRC_ICON="$ROOT/desktop/icons/icon.png"

echo "[bundle] building release binary…"
cargo build --release --manifest-path "$ROOT/desktop/Cargo.toml"
[ -x "$BIN" ] || { echo "error: release binary not found at $BIN" >&2; exit 1; }

echo "[bundle] assembling $APP_NAME.app…"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"

# .icns from the source PNG (flat color upscales cleanly to every size).
if command -v iconutil >/dev/null 2>&1 && [ -f "$SRC_ICON" ]; then
  ICONSET="$(mktemp -d)/$APP_NAME.iconset"
  mkdir -p "$ICONSET"
  for spec in "16:16x16" "32:16x16@2x" "32:32x32" "64:32x32@2x" \
              "128:128x128" "256:128x128@2x" "256:256x256" "512:256x256@2x" \
              "512:512x512" "1024:512x512@2x"; do
    px="${spec%%:*}"; name="${spec##*:}"
    sips -z "$px" "$px" "$SRC_ICON" --out "$ICONSET/icon_${name}.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"
else
  echo "[bundle] (no iconutil/icon source — bundling without a custom icon)"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Ubilink</string>
  <key>CFBundleDisplayName</key><string>Ubilink</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$APP_NAME</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- Required so macOS shows a reason on the Automation (Apple Events) prompt
       instead of killing the app when it scripts Safari/Finder/System Events. -->
  <key>NSAppleEventsUsageDescription</key>
  <string>ubilink reads the link (URL or file path) of the app you're in, to create a link.</string>
</dict>
</plist>
PLIST

# Validate the plist, then sign with a STABLE self-signed identity so macOS TCC
# (Accessibility/Automation) keeps the grant across rebuilds. Ad-hoc signing
# (`--sign -`) gives a fresh code-hash each rebuild with no stable "designated
# requirement", so TCC drops the grant whenever the binary changes. A self-signed
# cert — created once in the login keychain, reused forever — gives the app a
# consistent identity, so you grant permissions ONCE and they persist.
plutil -lint "$APP/Contents/Info.plist" >/dev/null

SIGN_IDENTITY="Ubilink Self-Signed"
if ! security find-certificate -c "$SIGN_IDENTITY" >/dev/null 2>&1; then
  echo "[bundle] creating a one-time self-signed code-signing identity ($SIGN_IDENTITY)…"
  # Build a code-signing cert non-interactively and import it into the login keychain.
  TMP="$(mktemp -d)"
  cat > "$TMP/req.cnf" <<CNF
[ req ]
distinguished_name = dn
prompt = no
x509_extensions = v3
[ dn ]
CN = $SIGN_IDENTITY
[ v3 ]
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
basicConstraints = critical, CA:false
CNF
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" -config "$TMP/req.cnf" >/dev/null 2>&1
  # -legacy + a transport password: Apple's `security` can't verify the PKCS12 MAC
  # that OpenSSL 3.x writes by default, and rejects an empty-password p12. A legacy
  # MAC with a throwaway password imports cleanly (the password only protects the
  # temp file, never stored).
  openssl pkcs12 -export -legacy -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -name "$SIGN_IDENTITY" -out "$TMP/id.p12" -passout pass:ubilink-bundle >/dev/null 2>&1
  # -T /usr/bin/codesign pre-authorizes codesign to use the key (first sign may show
  # a one-time keychain prompt — click "Always Allow").
  security import "$TMP/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
    -P "ubilink-bundle" -T /usr/bin/codesign >/dev/null 2>&1
  rm -rf "$TMP"
fi

if security find-certificate -c "$SIGN_IDENTITY" >/dev/null 2>&1; then
  codesign --force --sign "$SIGN_IDENTITY" "$APP"
  echo "[bundle] signed with stable identity '$SIGN_IDENTITY' — TCC grants persist across rebuilds."
else
  echo "[bundle] WARNING: self-signed identity unavailable; falling back to ad-hoc (grants reset on rebuild)."
  codesign --force --sign - "$APP"
fi

echo "[bundle] done: $APP"
echo "[bundle] grant Accessibility + Automation to THIS app once (drag it into the"
echo "         System Settings panes, or launch it and use the in-app onboarding)."
