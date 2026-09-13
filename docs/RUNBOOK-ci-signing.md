# Runbook — CI signed builds (O3)

The release signing pipeline (`ubilink_core::release` + `bin/ubilink-sign` +
`.github/workflows/release.yml`) produces ed25519-signed integration bundles in CI
and self-verifies them before upload. The crypto + wire format are proven headlessly
by unit tests in `src-tauri/src/release.rs` and an end-to-end CLI smoke. The **only**
ENV-blocked part is GitHub Actions executing the workflow with the org's signing
secret — this runbook enables that.

## What is already proven (no CI needed)
- `sign_to_json` → `verify_json` roundtrip; tamper → `BadSignature`; untrusted key
  → `UntrustedSigner`; malformed key/hex/JSON → typed `SignError` (7 unit tests).
- The `ubilink-sign` CLI end-to-end: `sign`/`pubkey`/`verify` exit 0 on a trusted
  artifact and exit 1 on an untrusted or tampered one.
- The real `plugins/examples/benign` manifest+source sign and verify through the CLI.

## One-time setup (a maintainer with repo admin)

1. **Generate the release signing key** (keep it OFF CI logs; this is the private key):
   ```sh
   openssl rand -hex 32        # 64 hex chars = the 32-byte ed25519 signing key
   ```
2. **Create the `release` environment** (the signing job declares
   `environment: release` and cannot start until it exists and is approved):
   repo → *Settings › Environments* → New environment → name `release`, then:
   - **Required reviewers:** add at least one maintainer (enable *Prevent
     self-review* if more than one maintainer exists).
   - **Deployment branches and tags:** *Selected branches and tags* → add the
     branch `main` and the tag pattern `v*`. (The workflow's job-level `if:` also
     refuses manual runs off `main` and non-`v*` tags; this makes GitHub enforce it.)
3. **Store the key as an environment secret (not a repo secret):** in the
   `release` environment → *Environment secrets* → Add secret → name
   `UBILINK_SIGNING_KEY`, value = the hex from step 1. If a repository-level
   `UBILINK_SIGNING_KEY` exists from an earlier setup, **delete it** (*Settings ›
   Secrets and variables › Actions*) so the key is reachable only through the gated
   environment.
4. **Protect release tags:** add a tag ruleset for `v*` (*Settings › Rules ›
   Rulesets*) restricting creation/update/deletion to maintainers, so only a
   maintainer can trigger a tag release.
5. **Record the PUBLIC key to pin in consumers.** Locally:
   ```sh
   UBILINK_SIGNING_KEY=<hex> cargo run -q --bin ubilink-sign -- pubkey
   ```
   This is the 64-hex public key an on-device installer must trust (`UBILINK_TRUSTED_KEYS`).
   It is safe to commit/publish.

## Triggering & verifying the pipeline
- Push a tag `vX.Y.Z` (or run the workflow manually via *Actions ›
  release-signed-build* **from `main`**; a manual run from any other branch is
  skipped by the job's `if:`).
- A reviewer from the `release` environment must approve the run before it starts.
- Expected: on `macos-latest` (same as `ci.yml`), with the pinned 1.92.0 toolchain
  and Node 22, the job runs `make verify LOCKED=--locked` (fmt + clippy + every test
  suite incl. the S2 dep guard and the macOS network-jail test), builds
  `ubilink-sign`, signs `dist/benign.signed.json`, **self-verifies** it (the build
  fails if the artifact doesn't validate), and uploads `signed-bundles`. No build
  cache is restored in this job, so expect a cold build (timeout: 90 min).
- Download the artifact and confirm locally:
  ```sh
  UBILINK_TRUSTED_KEYS=<published-pubkey> \
    cargo run -q --bin ubilink-sign -- verify --bundle benign.signed.json
  ```

## Recording the result
A green pipeline + a downloaded artifact that verifies against the published pubkey
closes **O3**. Update `.manifold/linking-manifesto-app.verify.json`
(O3 MISSING → SATISFIED) **only after** observing that green run — never from the
headless build, which keeps O3's runtime execution ENV-blocked.

## Hardening done
- Every `uses:` in `ci.yml` and `release.yml` is pinned to a full 40-char commit
  SHA with a `# vX.Y.Z` comment naming the release it was resolved from
  (`dtolnay/rust-toolchain` has no semver releases, so it is pinned to a
  master-branch commit and passed `toolchain: "1.92.0"` explicitly). To bump, resolve
  the new tag with `gh api repos/OWNER/REPO/git/ref/tags/<tag>` (dereference an
  annotated tag via `gh api repos/OWNER/REPO/git/tags/<sha>`) and update both files.
- The signing job is gated by the `release` environment, restricted to `main` /
  `v*`, runs `make verify` on macOS before signing, and restores no build cache.

## Hardening backlog (before real publishing)
- Let Dependabot (`package-ecosystem: github-actions`) propose SHA-pin bumps so
  the pins don't go stale.
- Consider OIDC keyless signing (Sigstore) so no long-lived private key lives in a
  secret; the `release` module's trust model already supports rotating trusted keys.
