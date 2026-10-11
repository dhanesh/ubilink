# Runbook: releasing Ubilink

How to cut a release of the macOS app. Design: `docs/superpowers/specs/2026-09-14-v0.1-release-design.md`.
Pipeline: `.github/workflows/release.yml`. Packaging: `scripts/bundle-macos.sh`, which
builds `Ubilink.app` from the GPUI app (`desktop-gpui/`, release profile, no
`automation` feature) into `desktop-gpui/bundle/`; its entitlements and icon live
in `desktop-gpui/packaging/`.

## One-time setup

### 1. Apple credentials (needs the paid Apple Developer account)

1. **Developer ID Application certificate.** In Xcode → Settings → Accounts → your
   team → Manage Certificates → `+` → *Developer ID Application*, or create a CSR in
   Keychain Access and upload it at developer.apple.com → Certificates. In Keychain
   Access, export the certificate **with its private key** as `DeveloperID.p12` and
   choose a strong export password.
2. **Signing identity string:** `security find-identity -v -p codesigning` — copy the
   full name, e.g. `Developer ID Application: Your Name (TEAMID1234)`.
3. **App Store Connect API key for notarization:** App Store Connect → Users and
   Access → Integrations → App Store Connect API → `+`, role **Developer**. Note the
   **Key ID** and **Issuer ID**; download `AuthKey_<KEYID>.p8` (downloadable once).
4. Base64-encode the two files: `base64 -i DeveloperID.p12 | pbcopy` and
   `base64 -i AuthKey_<KEYID>.p8 | pbcopy` (paste each into its secret below).

### 2. GitHub

1. **Settings → Environments → New environment `release`:** add yourself (and any
   co-maintainer) as **required reviewers**; under *Deployment branches and tags*
   choose *Selected branches and tags* and add branch `main` and tag pattern `v*`.
2. **Environment secrets** in `release` (Settings → Environments → release → Add secret):

   | Secret | Value |
   |---|---|
   | `APPLE_SIGNING_IDENTITY` | the identity string from step 1.2 |
   | `APPLE_CERTIFICATE_P12_BASE64` | base64 of `DeveloperID.p12` |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
   | `APPLE_API_KEY_ID` | the API Key ID |
   | `APPLE_API_ISSUER` | the Issuer ID |
   | `APPLE_API_KEY_P8_BASE64` | base64 of `AuthKey_<KEYID>.p8` |
   | `UBILINK_SIGNING_KEY` | the ed25519 integration signing key (`openssl rand -hex 32`); move it here from the repository secrets and delete the repo-level copy |

   All six `APPLE_*` secrets must be present together; a partial set fails the build.
3. **Settings → Rules → Rulesets → New tag ruleset:** target `v*`; restrict creation,
   update and deletion to maintainers.
4. **Settings → Security → Private vulnerability reporting:** enable (used by `SECURITY.md`).

## Cutting a release

1. **Bump the version** in `desktop-gpui/Cargo.toml` (the source of truth — the shipped
   GPUI app), `core/Cargo.toml`, `registry/Cargo.toml` and `app/Cargo.toml`; run
   `cargo check --manifest-path <each>/Cargo.toml` to update the lockfiles; add a
   `## [X.Y.Z] - unreleased` section to `CHANGELOG.md`. `make verify` (`verify-version`)
   fails if any of these disagree. Merge via a PR.
2. **Release candidate:** `git tag -a vX.Y.Z-rc.1 -m "Ubilink X.Y.Z-rc.1" && git push origin vX.Y.Z-rc.1`.
   Approve the `release` environment when GitHub asks. Without Apple secrets the DMG
   is `…-UNNOTARIZED.dmg`; with them it is notarized and stapled.
3. **Run the RC checklist** (below) on the draft prerelease's DMG. Fix and cut `-rc.2`…
   as needed.
4. **Final:** set the CHANGELOG heading's date (`## [X.Y.Z] - YYYY-MM-DD`) via a PR,
   then `git tag -a vX.Y.Z -m "Ubilink X.Y.Z" && git push origin vX.Y.Z`. A final tag
   **fails** without the Apple secrets. Review the draft release and **Publish** it.

## RC checklist

On a clean **Apple silicon** Mac and a clean **Intel** Mac:

- [ ] Download the DMG and `SHA256SUMS` from the draft; `shasum -a 256 -c SHA256SUMS` says `OK`.
- [ ] Opening the DMG and launching Ubilink from Applications shows **no** Gatekeeper
      warning (notarized builds only; an UNNOTARIZED dry run is expected to warn).
- [ ] `spctl -a -t exec -vv /Applications/Ubilink.app` → `accepted`, `source=Notarized Developer ID`.
- [ ] The Automation prompt appears, with the Apple Events reason text. No Accessibility prompt appears.
- [ ] Cmd+Shift+K in Safari captures the page; in Finder captures the open folder.
- [ ] Cmd+Shift+L lists links; opening one works; export then import round-trips.
- [ ] Settings shows `Ubilink X.Y.Z`.

## When something fails

- **Notarization rejected:** the build log prints `xcrun notarytool log <id>` output.
  Typical causes: a missing `--options runtime`, an unsigned nested binary, or an
  entitlement Apple rejects. Reproduce locally with the six variables exported and
  `make dist`.
- **`release-mode: … final release tag but no Apple credentials`:** add the secrets
  (setup step 2) or use an `-rc.N` tag.
- **`release-mode: tag vA does not match version B`:** the tag and `desktop-gpui/Cargo.toml`
  disagree; bump or re-tag.
- **Captures fail with error -1743 on a signed build:** the Apple Events entitlement
  is missing from the signature — check
  `codesign -d --entitlements - --xml /Applications/Ubilink.app`.
