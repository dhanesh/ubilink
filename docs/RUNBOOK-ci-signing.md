# Runbook — CI signed builds (O3)

The release signing pipeline (`hooky_core::release` + `bin/hooky-sign` +
`.github/workflows/release.yml`) produces ed25519-signed integration bundles in CI
and self-verifies them before upload. The crypto + wire format are proven headlessly
by unit tests in `src-tauri/src/release.rs` and an end-to-end CLI smoke. The **only**
ENV-blocked part is GitHub Actions executing the workflow with the org's signing
secret — this runbook enables that.

## What is already proven (no CI needed)
- `sign_to_json` → `verify_json` roundtrip; tamper → `BadSignature`; untrusted key
  → `UntrustedSigner`; malformed key/hex/JSON → typed `SignError` (7 unit tests).
- The `hooky-sign` CLI end-to-end: `sign`/`pubkey`/`verify` exit 0 on a trusted
  artifact and exit 1 on an untrusted or tampered one.
- The real `plugins/examples/benign` manifest+source sign and verify through the CLI.

## One-time setup (a maintainer with repo admin)

1. **Generate the release signing key** (keep it OFF CI logs; this is the private key):
   ```sh
   openssl rand -hex 32        # 64 hex chars = the 32-byte ed25519 signing key
   ```
2. **Store it as a secret:** repo → *Settings › Secrets and variables › Actions* →
   New repository secret → name `HOOKY_SIGNING_KEY`, value = the hex from step 1.
3. **Record the PUBLIC key to pin in consumers.** Locally:
   ```sh
   HOOKY_SIGNING_KEY=<hex> cargo run -q --bin hooky-sign -- pubkey
   ```
   This is the 64-hex public key an on-device installer must trust (`HOOKY_TRUSTED_KEYS`).
   It is safe to commit/publish.

## Triggering & verifying the pipeline
- Push a tag `vX.Y.Z` (or run the workflow manually via *Actions › release-signed-build*).
- Expected: the job runs `cargo test --locked` (incl. the S2 dep guard), builds
  `hooky-sign`, signs `dist/benign.signed.json`, **self-verifies** it (the build
  fails if the artifact doesn't validate), and uploads `signed-bundles`.
- Download the artifact and confirm locally:
  ```sh
  HOOKY_TRUSTED_KEYS=<published-pubkey> \
    cargo run -q --bin hooky-sign -- verify --bundle benign.signed.json
  ```

## Recording the result
A green pipeline + a downloaded artifact that verifies against the published pubkey
closes **O3**. Update `.manifold/linking-manifesto-app.verify.json`
(O3 MISSING → SATISFIED) **only after** observing that green run — never from the
headless build, which keeps O3's runtime execution ENV-blocked.

## Hardening backlog (before real publishing)
- Pin `dtolnay/rust-toolchain` / `actions/*` to commit SHAs (supply-chain).
- Consider OIDC keyless signing (Sigstore) so no long-lived private key lives in a
  secret; the `release` module's trust model already supports rotating trusted keys.
