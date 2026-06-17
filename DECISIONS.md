# Decisions

## D1 — Plugin sandbox = separate-process Node host under the Permission Model (Solution Option B)

**Date:** 2026-06-17 · **Constraints:** RT-5 (binding), RT-6, S1, T1, TN1, TN6, SC-3 · **Evidence:** E8/E9

Third-party plugins run in a separate `node --permission` process with no
`--allow-fs*` / `--allow-child-process` / `--allow-worker` flags. The Rust core is
the capability broker; all privileged access is broker-mediated over an IPC pipe.
Plugin source is streamed from the trusted core and loaded via a `data:` URL
import (so the host needs zero fs grants). Proven by `src-tauri/tests/sandbox.rs`
(4 e2e tests + 4 broker unit tests, all passing).

**Empirically verified (Node v24.16.0):**
- `fs.readFile`, `fs.writeFile`, `child_process.spawn`, `worker_threads`
  → `ERR_ACCESS_DENIED` (denied). ✅
- Broker denies every ungranted capability and the link graph wholesale. ✅
- Grants are revocable mid-session (SC-3). ✅

## D2 — ⚠️ Network is NOT contained by the Node Permission Model

**Verified:** under `node --permission` with no allow flags,
`fetch('http://example.com')` returns HTTP 200. The Node Permission Model does
**not** restrict outbound network in v24.

**Consequence:** S1's "no ambient network access" is **only partially** met by the
runtime mechanism. fs/child/worker are contained; **net is not**. We do not claim
net is sandboxed.

**Mitigation / future work:** OS-level network jail around the host process —
macOS `sandbox-exec` deny-network profile, Linux seccomp-bpf / net namespaces,
Windows AppContainer — plus routing any granted network capability through the
broker, plus provenance gating (signing, S2) on plugin install. Tracked as a
follow-up before any plugin is allowed to declare a network capability.

## D3 — Plugin shares the host realm (partial integrity hardening applied)

**Date:** 2026-06-17 · **Constraints:** S1, TN1 · **Source:** m4 code-quality review

The plugin is loaded via `data:` import into the host's own Node process/realm, so
it shares `globalThis`/`process`. The OS-enforced ambient denial (fs/child/worker)
holds regardless, but a hostile plugin could otherwise tamper with the protocol
channel.

**Applied now:** `process.stdout.write` is neutered (rerouted to stderr) after the
host captures the real writer, so a plugin **cannot forge protocol frames** on
stdout. Regression-guarded by escape-7 in `plugins/examples/malicious/plugin.mjs`
+ an assertion in `fail_closed_no_fs_no_child_escape`.

**Deferred to the integration-runtime task:** full realm isolation — run the
plugin in a curated `node:vm` SourceTextModule context with **no** `process`
handle (so it cannot touch stdin, `process.exit`, env, or fds), or one host
process *per* plugin. The current spike proves the binding constraint; complete
isolation is a runtime-design task, not a spike concern.

## D4 — No plugin execution timeout / IPC line cap yet (DoS — deferred)

**Date:** 2026-06-17 · **Constraints:** S5, GAP-10 (resource exhaustion) · **Source:** m4 code-quality review

`Session::read_msg` blocks on `read_line` with no timeout or size cap. A plugin
that hangs in `run()` or emits an unbounded line can stall or OOM the host.

**Deferred to the integration-runtime task:** wrap host reads in a reader thread
with `recv_timeout` + child-process kill on timeout, and cap line length. Maps to
S5/GAP-10. Acceptable in the spike (trusted local examples); **must** land before
arbitrary third-party plugins run.
