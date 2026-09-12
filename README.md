# Ubilink

An open-source, cross-platform desktop app for **ubiquitous linking** — a faithful,
extensible implementation of the [Linking Manifesto](https://linkingmanifesto.org/).

> *"The ability to copy a link to a resource is as important for cognitive
> productivity as the ability to copy other types of information."* — the Linking Manifesto

Ubilink lets you copy a robust link to whatever resource is active in *any* supported
app, link resources together into a **bidirectional graph**, and navigate that graph —
all locally, privately, and extensibly. It is a clean-room, open implementation of ubiquitous linking.

## What it does (the manifesto's two asks)

1. **UI** — obtain a link to the currently open/selected resource via a hotkey panel.
2. **API** — a programmatic way to obtain or construct a link (its address + name).

Plus what makes it a *tool*: a stored, bidirectional link graph (open A → see B as a
backlink, and vice-versa), best-effort link survival when resources move, and a
third-party integration system so anyone can add support for new apps.

## Architecture

| Layer | Tech | Why |
|-------|------|-----|
| Core | **Rust** (Tauri) | OS automation, the link graph, the capability broker — the trusted domain |
| UI | Web (Tauri webview) | the hotkey panel + settings |
| Plugins | **Separate-process JS sandbox** | third-party integrations run with **no ambient fs/net/child** access; all privileged access is broker-mediated |
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
plugin wholesale — a plugin only ever sees the single resource it was invoked for.

> **Honest caveat:** the Node permission model does **not** yet contain outbound
> network. See [`DECISIONS.md`](DECISIONS.md) (D2) for the mitigation path. fs /
> child_process / worker access **are** provably denied.

> **Custom adapters are not plugins.** App adapters you add in `adapters.json`
> ([`docs/SUPPORTED-APPS.md`](docs/SUPPORTED-APPS.md)) are trusted local
> configuration: their AppleScript runs via `osascript` with **your** user
> privileges and Ubilink's Automation/Accessibility grants, and is **not**
> sandboxed. The guarantees above apply only to the Node plugin runtime. Only use
> adapter scripts you wrote or have read. See [`DECISIONS.md`](DECISIONS.md) (D14).

## Status

Early foundation. Proven and tested so far:

- ✅ Plugin sandbox + capability broker (the security keystone) — `src-tauri/`, `plugins/`
- ✅ Resource identity + confidence-gated link re-resolution — `src-tauri/src/resource.rs`
- ✅ Bidirectional SQLite link graph (backlinks, idempotent, concurrent-safe) — `src-tauri/src/graph.rs`
- ✅ Per-app/-platform capability matrix with honest degradation — `src-tauri/src/capabilities.rs`
- ⬜ Hotkey panel UI · macOS link adapters · integration runtime/installer (next)

## Build & test

```sh
# Rust core (broker, graph, resource identity, capabilities)
cd src-tauri && cargo test

# the plugin sandbox proof spawns a real `node --permission` child process
node --version   # v24+ required for the Node Permission Model
```

## Extending Ubilink

Add support for a new app by writing an **integration**: a manifest (declaring the
capabilities it needs) plus sandboxed JS. See [`plugins/README.md`](plugins/README.md)
and the manifest schema at [`schema/integration.manifest.schema.json`](schema/integration.manifest.schema.json).

## License

[MIT](LICENSE) — build on it freely, including commercial integrations.
