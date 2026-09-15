# Ubilink

An open-source desktop app for **ubiquitous linking** — a faithful, extensible
implementation of the [Linking Manifesto](https://linkingmanifesto.org/). It is
designed to be cross-platform, but today it builds and captures on **macOS only**
(see [Status](#status)).

> *"The ability to copy a link to a resource is as important for cognitive
> productivity as the ability to copy other types of information."* — the Linking Manifesto

Ubilink lets you copy a robust link to whatever resource is active in *any* supported
app, link resources together into a **bidirectional graph**, and navigate that graph —
all locally, privately, and extensibly. It is a clean-room, open implementation of ubiquitous linking.

## What it does (the manifesto's two asks)

1. **UI** — obtain a link to the currently open/selected resource via a hotkey panel.
2. **API** — a programmatic way to obtain or construct a link (its address + name).

Plus what makes it a *tool*: a stored, bidirectional link graph (open A → see B as a
backlink, and vice-versa), best-effort link survival when resources move, and app
adapters you can extend yourself. A sandboxed third-party integration system is built
and tested, and is coming to the app in a later release.

## Architecture

| Layer | Tech | Why |
|-------|------|-----|
| Core | **Rust** (Tauri) | OS automation, the link graph, the capability broker — the trusted domain |
| UI | Web (Tauri webview) | the hotkey panel + settings |
| Plugins | **Separate-process JS sandbox** | third-party integrations run with **no ambient fs/child** access (and, on macOS, no network); all privileged access is broker-mediated |
| Storage | **Local SQLite** | the bidirectional graph, on-device, zero telemetry |

Key decisions and their rationale live in [`DECISIONS.md`](DECISIONS.md). The full
constraint model (constraints, resolved tensions, required truths) is in
[`.manifold/`](.manifold/) — this project was built constraint-first with the
[Manifold](https://github.com/dhanesh/manifold) framework.

### Security model (why plugins can't hurt you)

Third-party integrations are **untrusted**. They run in a separate `node --permission`
process that denies filesystem, child-process, and worker access at the OS/runtime
level. The only thing a plugin can do is request a capability it declared in its
manifest; the **Rust capability broker** checks the grant and either performs the
mediated action or denies it (deny-by-default). The link graph is never exposed to a
plugin wholesale — a plugin only ever sees the single resource it was invoked for,
and only if its manifest declares `read:focused-resource` (the broker is the only
way to get it, and revoking the grant stops it).

> **Honest caveat:** the Node permission model alone does **not** contain outbound
> network. On macOS the plugin host also runs in a `sandbox-exec` jail that denies
> network, file writes and signals to other processes; on Linux/Windows that jail is
> not wired yet. See [`DECISIONS.md`](DECISIONS.md) (D1, D2).

> **Custom adapters are not plugins.** App adapters you add in `adapters.json`
> ([`docs/SUPPORTED-APPS.md`](docs/SUPPORTED-APPS.md)) are trusted local
> configuration: their AppleScript runs via `osascript` with **your** user
> privileges and Ubilink's Automation/Accessibility grants, and is **not**
> sandboxed. The guarantees above apply only to the Node plugin runtime. Only use
> adapter scripts you wrote or have read. See [`DECISIONS.md`](DECISIONS.md) (D14).

## Status

Built and tested:

- ✅ macOS desktop app: hotkey panel (Cmd+Shift+K capture, Cmd+Shift+L link browser,
  Cmd+Shift+, settings), permission onboarding, graph export/import, app-adapter
  manager — `desktop/`, `dist/`
- ✅ Declarative macOS app adapters (7 browsers, Finder, Bear, Things3), extensible
  with your own `adapters.json`; every `osascript` call is bounded (15 s) —
  `src-tauri/src/adapters_config.rs`, `src-tauri/src/live_capture.rs`,
  [`docs/SUPPORTED-APPS.md`](docs/SUPPORTED-APPS.md)
- ✅ Link validation: one policy for capture, import and open (scheme deny-list,
  local-only `file:`), and openers that never go through a shell — `src-tauri/src/api.rs`,
  `src-tauri/src/adapter.rs`
- ✅ Bidirectional SQLite link graph (backlinks, idempotent, concurrent-safe,
  versioned atomic migrations) — `src-tauri/src/graph.rs`
- ✅ Resource identity + confidence-gated link re-resolution — `src-tauri/src/resource.rs`
- ✅ Plugin sandbox + capability broker (the security keystone) — `src-tauri/src/sandbox.rs`,
  `src-tauri/src/broker.rs`, `plugins/`
- ✅ Signed integrations (ed25519 over the full manifest, schema gate, downgrade
  protection), the `ubilink-sign` CLI and a CI signing workflow —
  `src-tauri/src/integrations.rs`, `src-tauri/src/release.rs`
- ✅ Integration registry service (std-only HTTP, per-id publisher ownership, bounded
  requests) — `registry/`
- ✅ Capability matrix derived from the adapter registry, published as
  [`docs/CAPABILITIES.md`](docs/CAPABILITIES.md) and drift-guarded by a test —
  `src-tauri/src/capabilities.rs`

Not done yet:

- ⬜ **Windows/Linux capture is unsupported.** Every adapter captures through macOS
  AppleScript; off macOS only a file path can be linked
  ([`docs/CAPABILITIES.md`](docs/CAPABILITIES.md)). The desktop app is built and
  CI-tested on macOS only.
- ⬜ **The plugin sandbox runtime is not wired into the desktop app.** The app does not
  install or run integrations yet; the sandbox, broker and installer are exercised by
  tests and the CLI tools.
- ⬜ **No OS sandbox off macOS.** On Linux/Windows the plugin host has no jail, so
  network, native file writes and signals are open there (`DECISIONS.md` D1).
- ⬜ **No hosted registry.** The registry service persists its index and each id's
  owner to a data directory (`UBILINK_REGISTRY_DATA`), but no public, governed
  instance runs yet ([`docs/RUNBOOK-registry.md`](docs/RUNBOOK-registry.md)).

## Install

**Requirements:** macOS 10.15 or later, on Apple silicon or Intel.

1. Download the latest `Ubilink-<version>-universal.dmg` and `SHA256SUMS` from
   [Releases](https://github.com/dhanesh/ubilink/releases).
2. Check the download: `shasum -a 256 -c SHA256SUMS` (the `.dmg` line should say `OK`).
3. Open the DMG and drag **Ubilink** to **Applications**, then launch it.

**First run:** Ubilink asks for two macOS permissions — **Automation** (so it can read
the link of the app you're in) and **Accessibility** (for the global shortcuts). The
in-app onboarding walks you through both. If you ran a development build before, macOS
asks once more for the released app.

Press **Cmd+Shift+K** to capture, **Cmd+Shift+L** to browse your links, and
**Cmd+Shift+,** for settings.

**Uninstall:** quit Ubilink, delete it from Applications, and delete
`~/Library/Application Support/org.dhanesh.ubilink` to remove your link database.

## Build & test

Requires macOS, the Rust toolchain pinned in `rust-toolchain.toml`, and Node ≥ 22.13.

```sh
make verify                  # the merge gate: fmt + clippy + every crate's tests + JS tests
make verify LOCKED=--locked  # exactly what CI runs
make test                    # faster loop: core + registry + JS (skips the Tauri build)
make run                     # build + launch the desktop app
```

**Node:** the sandbox tests spawn a real `node --permission` process, which needs
Node ≥ 22.13 (CI uses Node 22). The plugin host runs with an empty environment, so
the core resolves `node` to an absolute path from the absolute entries on your
`PATH`. The first `node` found there must be a real binary: a version-manager shim
that needs `PATH`/`HOME` to find the real `node` will fail.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full workflow.

## Extending Ubilink

Today you add support for a new app with an **app adapter**: a JSON entry in your own
`adapters.json` (see [`docs/SUPPORTED-APPS.md`](docs/SUPPORTED-APPS.md)). Adapters are
trusted local config that run with your permissions.

Sandboxed third-party **integrations** — a manifest declaring the capabilities they
need plus JS that runs in the Node sandbox — are built and tested but not yet enabled
in the app. See [`plugins/README.md`](plugins/README.md) and the manifest schema at
[`schema/integration.manifest.schema.json`](schema/integration.manifest.schema.json).

## License

[MIT](LICENSE) — build on it freely, including commercial integrations.
