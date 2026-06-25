# ubilink plugin sandbox + capability broker

> Spike proving the plugin-isolation architecture (Solution Option B). Satisfies
> **RT-5** (binding), **RT-6**, **S1**, **T1**, **TN1**, **TN6** (the isolation
> proof), **SC-3**. See `.manifold/linking-manifesto-app.md`.

## The model in one paragraph

Third-party plugin code is **untrusted** and never runs in the core's trust
domain (T1). It runs in a **separate Node process** launched with the **Node
Permission Model** (`node --permission`) and **no** `--allow-fs*` /
`--allow-child-process` / `--allow-worker` flags — so the runtime itself denies
ambient filesystem, child-process and worker-thread access (deny-by-default, S1).
The plugin's **only** channel to the outside world is an IPC pipe to the Rust
**capability broker**, which checks every request against the capabilities the
plugin declared in its manifest (RT-6) and either performs the mediated action or
denies it (TN1). The link graph is never exposed wholesale: the only read handler
returns the single focused resource the plugin was invoked for.

```
┌────────────────────────┐         IPC: newline-delimited JSON        ┌─────────────────────────────┐
│  Rust core (TRUSTED)   │  ────────────  stdin/stdout  ───────────►  │  node --permission (UNTRUSTED) │
│  src-tauri/            │                                            │  plugins/runtime/             │
│  • reads bundle+manifest                                            │    plugin-host.mjs            │
│    from disk (full fs) │  ◄── cap_request ────────────────────────  │  • loads plugin SOURCE via    │
│  • broker.rs: decide() │  ─── cap_response (allow/deny) ──────────►  │    data: URL (no fs needed)   │
│  • streams SOURCE in   │                                            │  • hands plugin ONLY caps.*   │
└────────────────────────┘                                            │  • fs/child/worker => DENIED  │
                                                                      └─────────────────────────────┘
```

Why stream the source instead of letting the host `import('./plugin.mjs')`?
Because the ESM file-resolver walks the tree and needs broad `--allow-fs-read=*`,
which would also hand fs-read to the plugin (permissions are process-wide). The
trusted Rust core reads the bundle (it has full fs rights) and streams the source
over the pipe; the host loads it via a `data:` URL import, which touches no
filesystem and so needs zero fs grants. Verified: code loaded this way still
cannot `fs.readFile`.

## Capability-string grammar

```
<verb>:<scope>
```

- `verb` — a lowercase action: `open`, `read`, …
- `scope` — what the verb applies to.
- A **scheme-prefix** scope ending in `://` authorizes any request beginning with
  it. Grant `open:obsidian://` authorizes `open:obsidian://vault/Note.md`.
- Otherwise scopes match **exactly**. Grant `read:focused-resource` authorizes
  only `read:focused-resource`.
- **No wildcards.** There is deliberately no capability string that yields the
  link graph wholesale. Even an adversarial manifest asking for `read:link-graph`,
  `read:graph`, or `read:*` is denied — the broker has no handler that returns a
  graph (S1).

Examples:

| Grant                       | Request                          | Result |
|-----------------------------|----------------------------------|--------|
| `open:obsidian://`          | `open:obsidian://vault/Note.md`  | allow  |
| `open:obsidian://`          | `open:file:///etc/passwd`        | deny (scheme not granted) |
| `read:focused-resource`     | `read:focused-resource`          | allow → single resource |
| `read:focused-resource`     | `read:link-graph`                | deny (deny-by-default) |
| `read:link-graph` (declared)| `read:link-graph`                | deny (no broker handler) |

## Layout

| Path | Role |
|------|------|
| `runtime/plugin-host.mjs` | the deny-by-default Node sandbox host |
| `examples/benign/`        | well-behaved plugin: uses only granted caps |
| `examples/malicious/`     | attacker: tries fs/child/worker/graph escapes |
| `../schema/integration.manifest.schema.json` | manifest JSON Schema (RT-6) |
| `../src-tauri/src/broker.rs` | the capability broker (decide/revoke) |
| `../src-tauri/src/sandbox.rs` | spawns the host, owns the IPC |
| `../src-tauri/tests/sandbox.rs` | end-to-end proof (E8/E9) |

## ⚠️ Honest limitation: network is NOT sandboxed by this mechanism

Verified empirically in this environment (Node v24.16.0): under `node --permission`
with no allow flags, `fetch('http://example.com')` **succeeds**. The Node
Permission Model does **not** restrict outbound network. We therefore do **not**
claim network is contained by the sandbox runtime alone.

What IS provably denied by `--permission`: filesystem read/write, `child_process`,
and `worker_threads` (each throws `ERR_ACCESS_DENIED`). What the broker denies:
every capability not in the granted manifest, and the link graph wholesale.

**Hardening path (future work):** wrap the plugin host in an OS-level network
jail — macOS `sandbox-exec` (seccomp/`network*` deny profile) / Linux seccomp-bpf
or network namespaces / Windows AppContainer — so outbound sockets are blocked at
the kernel, and route any granted network capability through the broker. Until
then, treat network egress from a plugin as possible and gate plugin installation
on provenance (signing, S2).

## Run the proof

```
cd src-tauri && cargo test
# fail-closed escape demonstration:
cargo test fail_closed -- --nocapture --test-threads=1
```
