# ubilink plugin sandbox + capability broker

> Spike proving the plugin-isolation architecture (Solution Option B). Satisfies
> **RT-5** (binding), **RT-6**, **S1**, **T1**, **TN1**, **TN6** (the isolation
> proof), **SC-3**, **S5** (DoS bounds). See `.manifold/linking-manifesto-app.md`
> and `DECISIONS.md` D1–D4.

## The model in one paragraph

Third-party plugin code is **untrusted** and never runs in the core's trust
domain (T1). It runs in a **separate Node process** launched with the **Node
Permission Model** (`node --permission`) and **no** `--allow-fs*` /
`--allow-child-process` / `--allow-worker` flags, so the runtime itself denies
ambient filesystem, child-process, worker-thread and inspector access
(deny-by-default, S1). On macOS that process is also wrapped in a
**`sandbox-exec` jail** that denies outbound network, every file write, and
signals to other processes at the kernel. It starts with an **empty
environment**. The plugin's **only** channel to the outside world is an IPC pipe
to the Rust **capability broker**. The broker checks every request against the
capabilities the plugin declared in its manifest (RT-6), then either performs
the mediated action or denies it (TN1). The link graph is never exposed
wholesale: the only read handler returns the single focused resource the plugin
was invoked for, and only to a plugin that declared `read:focused-resource`.

## Plugin contract

A plugin exports `async function run({ probe, caps })`:

- `caps.request(capability)` resolves to `{ allowed, result, reason }`, and the
  broker decides each call when it is made. It is the **only** way to reach
  anything outside the plugin, including the focused resource:
  `await caps.request('read:focused-resource')` returns the single resource the
  plugin was invoked for if the manifest declares that capability and it has
  not been revoked. Otherwise it returns `allowed: false` and no data.
- The focused resource is **never** passed to `run()` and never crosses the pipe
  without a grant. (Before 2026-09-13 it was pushed as `run({ resource })` to
  every plugin, regardless of its manifest.)
- `probe` is optional caller-supplied input, passed through without brokering.
  The core never puts graph or resource data in it; today only the test
  harness uses it, to steer the hostile fixtures.

```
┌────────────────────────┐         IPC: newline-delimited JSON        ┌──────────────────────────────────┐
│  Rust core (TRUSTED)   │  ────────────  stdin/stdout  ───────────►  │ sandbox-exec jail (macOS)         │
│  core/                 │   nonce (1st line, once) · load · invoke   │  node --permission (UNTRUSTED)    │
│  • reads bundle+manifest                                            │  env = {} · cwd = /               │
│    from disk (full fs) │  ◄── "<nonce> " + cap_request / result ──  │  plugins/runtime/plugin-host.mjs  │
│  • drops frames w/o the│  ─── cap_response (allow/deny) ──────────►  │  • loads plugin SOURCE via data:  │
│    session nonce       │                                            │  • hands plugin ONLY caps.*       │
│  • broker.rs: decide() │                                            │  • fs/child/worker/net/writes/    │
│  • deadline watchdog   │                                            │    foreign signals => DENIED      │
└────────────────────────┘                                            └──────────────────────────────────┘
```

Why stream the source instead of letting the host `import('./plugin.mjs')`?
Because the ESM file-resolver walks the tree and needs broad `--allow-fs-read=*`,
which would also hand fs-read to the plugin (permissions are process-wide). The
trusted Rust core reads the bundle (it has full fs rights) and streams the source
over the pipe; the host loads it via a `data:` URL import, which touches no
filesystem and so needs zero fs grants. Verified: code loaded this way still
cannot `fs.readFile`.

## What is guaranteed (each item has a hostile-plugin test in `core/tests/sandbox.rs`)

| Guarantee | Enforced by | Test |
|-----------|-------------|------|
| no fs read/write, `child_process`, `worker_threads` | `node --permission` | `fail_closed_no_fs_no_child_escape` |
| no outbound network | macOS jail `(deny network*)` | `net_is_jailed_when_supported` |
| no native file writes (e.g. `trace_events` → `node_trace.N.log`) | host disables `trace_events` + jail `(deny file-write*)` | `plugin_cannot_write_files_natively` |
| cannot signal the core or any other process | jail `(deny signal)(allow signal (target self))` | `plugin_cannot_signal_the_core` |
| no inherited env secrets; `NODE_OPTIONS` cannot re-grant permissions | `env_clear()`; `node` resolved to an absolute path by the core | `child_env_is_scrubbed` |
| cannot forge protocol frames | per-session nonce on every frame (below) | `forged_frames_are_rejected` |
| a lying host cannot flip a capability verdict | the core uses its own broker's decision | `recheck_verdict_comes_from_the_broker_not_the_host` |
| hung, chatty or stdin-stalling plugins cannot hang the core | overall deadline + watchdog kill | `hung_plugin_is_killed`, `chatty_plugin_is_killed_at_overall_deadline`, `flooding_plugin_cannot_block_core_writes` |
| no leaked or zombie children | kill + reap on every failure and in `Drop` | `timed_out_child_is_reaped`, `dropping_session_kills_and_reaps_child` |
| only granted capabilities; never the whole graph | broker | `broker_allows_granted_denies_ungranted`, `no_wholesale_graph_only_focused_resource` |
| focused resource only with `read:focused-resource`, never after revocation, never pushed into `run()` or onto the pipe | broker is its only channel; `invoke` carries no resource | `undeclared_plugin_never_sees_the_focused_resource`, `declared_plugin_gets_the_focused_resource_from_the_broker`, `revoked_grant_stops_delivering_the_focused_resource` |

On macOS the OS-jail tests **fail** (rather than skip) if the jail is not
active, so the proof can't pass vacuously.

## Frame integrity: the per-session nonce

The plugin shares the host's JS realm, so it can write to fd 1 and monkeypatch
any global. Stubbing `process.stdout.write` is therefore not a boundary. Instead:

1. The core generates a random 64-hex-char nonce per session and sends it as
   the **first stdin line**. It never resends it and never puts it in argv or env.
2. The host, **before any plugin code loads**, captures `fs.readSync`/`fs.writeSync`/`JSON.stringify`,
   disables heap/trace introspection (`v8.getHeapSnapshot`, `queryObjects`,
   `setFlagsFromString`, `trace_events`), then reads the nonce line with the
   captured `readSync(0)` and keeps it only in module scope.
3. Every host frame is `"<nonce> " + JSON + "\n"`. It is built from a
   null-prototype object and written with the captured `writeSync(1, …)`.
4. The core drops every stdout line without the exact nonce. It also treats the
   payload of authenticated frames as plugin-controlled, and bases every
   decision on its own broker.

**Accepted residual:** after the nonce line, the plugin can read its own stdin.
That covers its own source, its `invoke` arguments (`probe` only, never the
focused resource), and the broker's replies to **its own** requests. One host
process serves one plugin, so this
reveals nothing it could not request anyway. It can also subvert host logic
inside its own realm, which only affects its own session. See `DECISIONS.md` D3.

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
| `examples/malicious/`     | attacker: tries fs/child/worker/net/graph escapes |
| `examples/hostile/`       | attacker: frame forgery, env/signal probes, native writes, DoS, focused-resource disclosure incl. a stdin tap (`probe.mode`) |
| `examples/hang/`          | never returns (timeout fixture) |
| `../schema/integration.manifest.schema.json` | manifest JSON Schema (RT-6) |
| `../core/src/broker.rs` | the capability broker (decide/revoke) |
| `../core/src/sandbox.rs` | spawns + jails the host, owns the authenticated IPC, deadlines |
| `../core/tests/sandbox.rs` | end-to-end proof (E8/E9) against a real `node` |

## Residual risk and follow-ups

- **Non-macOS has no OS jail yet.** Network, native file writes and signals to
  other processes are open there, and `Session::net_jailed()` reports `false`.
  The follow-ups are Linux landlock/seccomp-bpf/network namespaces and Windows
  AppContainer. Until then, gate plugin installation on provenance (signing, S2).
- **The macOS profile is `allow default` plus targeted denies, not deny-default.**
  Native code (a Node/V8 bug) could still read user-readable files. A
  deny-default profile needs a per-install allowlist for the node binary and
  its dylibs.
- **No memory cap.** A plugin can grow its heap until the deadline kills it.
- **`node` must be a real binary** found on an absolute `PATH` entry.
  Version-manager shims that need `PATH`/`HOME` do not work under the empty
  environment.
- History: the Node Permission Model alone does **not** restrict network. Under
  `node --permission`, `fetch('http://example.com')` succeeds (Node v24.16.0).
  Only the OS jail closes it (D2).

## Run the proof

```
cd core && cargo test --test sandbox
# fail-closed escape demonstration:
cargo test fail_closed -- --nocapture --test-threads=1
```
