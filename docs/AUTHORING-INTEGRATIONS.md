# Authoring a ubilink integration (B3)

You can add linking support for a new app **without forking or rebuilding ubilink's
core**. An integration is an installable bundle: a manifest + a small JS plugin,
signed by you. The core loads it through the same sandbox + capability broker every
integration uses — your code never runs in the core's trust domain (S1/T1).

This is constraint **B3**: the ecosystem, not the core team, scales app coverage.

## What an integration is

```
my-integration/
  manifest.json   # what the integration is + the capabilities it needs
  plugin.mjs      # the JS that runs (sandboxed) to capture/open links
```

- **manifest.json** — declares `id`, `name`, the target `app`, a `version`, and the
  `needs[]` capability strings. Schema: `schema/integration.manifest.schema.json`.
- **plugin.mjs** — runs in a separate `node --permission` process with **no** fs /
  child-process / worker access. Its only channel out is the capability broker; it
  may use exactly the capabilities it declared in `needs[]` (RT-6). See
  `plugins/README.md` for the runtime model and the capability-string grammar.

## Capabilities

`needs[]` entries are `verb:scope` strings, e.g.:

| Capability | Meaning |
| --- | --- |
| `open:obsidian://` | open any `obsidian://…` URL (scheme-prefix grant) |
| `read:focused-resource` | read the single resource the plugin was invoked for, via `await caps.request('read:focused-resource')` (it is **not** passed to `run()`; without this grant the plugin never sees it) |

There are **no wildcards** and no capability that yields the link graph wholesale —
even `read:link-graph` / `read:*` is denied (S1). Declare the least you need.

## Author workflow (no core changes)

1. **Write** `manifest.json` and `plugin.mjs` (copy `plugins/examples/benign/` as a
   starting point).
2. **Validate** the manifest before you ship. This enforces
   `schema/integration.manifest.schema.json`: a lowercase reverse-DNS `id`
   (`com.example.app`), a semver `version` (`1.0.0`), `verb:scope` needs with a
   lowercase verb and no duplicates, and no fields beyond the schema's:
   ```sh
   cargo run -q --bin ubilink-sign -- validate --manifest manifest.json
   ```
   The same check runs again at signing, at registry publish and at install.
3. **Sign** the bundle with your publisher key (a 64-hex ed25519 key; the same key
   model the app verifies against — see `docs/RUNBOOK-ci-signing.md`):
   ```sh
   UBILINK_SIGNING_KEY=<your-hex-key> \
     cargo run -q --bin ubilink-sign -- sign \
       --manifest manifest.json --source plugin.mjs --version 1 \
       > my-integration.signed.json
   ```
   There are two version numbers: the manifest's semver `version` is the
   human-facing label, and `--version` is the integer release number that must
   increase with every release (downgrade protection and the registry's
   monotonicity check use it). A registry binds your integration `id` to the key
   that first publishes it, so sign every release with the same key.
4. **Install** — ubilink's `Installer` accepts `my-integration.signed.json` only after
   verifying the signature, the trusted publisher, the manifest schema and version
   monotonicity (R3 / S3). No rebuild of ubilink is involved. *Not wired yet:* the
   desktop app has no install flow and does not run integrations, so today this path
   is exercised by tests, `ubilink-sign verify` and the registry.

## Why this satisfies B3 without touching core

Every piece above is already in the core as a *generic* mechanism: the manifest
schema, the validator (`Manifest::validate`), the broker (`broker.rs`), the sandbox
(`sandbox.rs`), the signer (`ubilink-sign`), and the installer (`integrations.rs`).
Adding an app exercises these — it never edits them. The supported-app set
(`docs/CAPABILITIES.md`, O2) grows by installation, not by a core release.
