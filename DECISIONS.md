# Decisions

## D1 — Plugin sandbox = separate-process Node host under the Permission Model (Solution Option B)

**Date:** 2026-06-17 · **Constraints:** RT-5 (binding), RT-6, S1, T1, TN1, TN6, SC-3 · **Evidence:** E8/E9

Third-party plugins run in a separate `node --permission` process with no
`--allow-fs*` / `--allow-child-process` / `--allow-worker` flags. The Rust core is
the capability broker; all privileged access is broker-mediated over an IPC pipe.
Plugin source is streamed from the trusted core and loaded via a `data:` URL
import (so the host needs zero fs grants). Proven end-to-end against a real
`node` by `src-tauri/tests/sandbox.rs` (hostile fixtures in
`plugins/examples/{malicious,hostile,hang}`).

**Containment layers (hardened 2026-09-13; each one has a hostile-plugin test):**
- **Node Permission Model:** `fs` read/write, `child_process`, `worker_threads`,
  `inspector`, `process.binding` and WASI → `ERR_ACCESS_DENIED`. ✅
- **macOS OS jail** (`sandbox-exec -p SANDBOX_PROFILE`, `src-tauri/src/sandbox.rs`):
  `(allow default)(deny network*)(deny file-write*)(deny signal)(allow signal (target self))`.
  Denies outbound network (D2), **every file write** (including native writes
  that bypass `--permission`, e.g. `trace_events` creating `node_trace.N.log` in
  the cwd), and **signals to any other process** (`process.kill(process.ppid, 0)`
  → `EPERM`; the permission model does not gate `kill`). Two traps found while
  hardening: `(deny signal (target others))` is *not* enforced by current macOS,
  and the old availability probe (`sandbox-exec -n no-network`) is killed on
  current macOS, which had **silently disabled the jail**. The probe now runs the
  exact profile, and on macOS the tests fail if the jail is not active.
- **Empty environment:** the host is spawned with `env_clear()`, so no core
  secret (e.g. `UBILINK_SIGNING_KEY` in CI) is visible and an inherited
  `NODE_OPTIONS` (verified: `--allow-fs-read=*` re-granted fs reads) cannot
  re-grant permissions or preload code. `node` is resolved by the core to an
  absolute canonical path (absolute `PATH` entries only); `sandbox-exec` is
  `/usr/bin/sandbox-exec`; the child's cwd is `/`.
- **Broker:** denies every ungranted capability and the link graph wholesale;
  grants are revocable mid-session (SC-3). ✅ The broker is also the **only**
  channel for the focused resource. `Session::invoke` sends
  `{"t":"invoke","probe":…}` and never the resource, and the host calls
  `run({ probe, caps })`. A plugin gets the resource only through
  `caps.request('read:focused-resource')`, which the broker decides at request
  time, so an undeclared grant yields nothing, a revoked grant stops working
  from the next request on, and every read lands in `cap_log`. Before
  2026-09-13 the core pushed the resource into `run({ resource })` without
  asking the broker, so any plugin saw it whatever it had declared. Proven by
  `tests/sandbox.rs::{undeclared_plugin_never_sees_the_focused_resource,
  declared_plugin_gets_the_focused_resource_from_the_broker,
  revoked_grant_stops_delivering_the_focused_resource}`. These tests also scan
  everything the plugin can read off its stdin, so the resource never crosses
  the pipe without a grant. `probe` is caller-supplied input that is passed
  through without brokering; callers may put in it only data any plugin may
  see (today only the test harness sets it).
- **Protocol integrity** (D3) and **DoS bounds** (D4).

**Residual risk / follow-ups:**
- Non-macOS has no OS jail yet (network, native file writes and signals are
  open there); `Session::net_jailed()` reports it. Linux landlock/seccomp and
  Windows AppContainer are follow-ups.
- The macOS profile is `allow default` plus targeted denies, not deny-default:
  native code (e.g. a V8/Node bug) could still read user-readable files or use
  mach services. A deny-default profile needs a per-install allowlist for the
  node binary and its dylibs; tracked as a follow-up.
- No memory cap: a plugin can grow its heap until the deadline kills it (add
  `--max-old-space-size` / an RLIMIT as a follow-up).
- `node` must be a real binary: version-manager shims that need `PATH`/`HOME`
  fail under the empty environment.

## D2 — Network containment: ✅ CLOSED via macOS sandbox-exec net jail

> **Resolved 2026-06-17 (build loop R1).** The host is now spawned wrapped in
> `sandbox-exec -p '(version 1)(allow default)(deny network*)…'` on macOS (the
> full profile, which also denies file writes and foreign signals, is in D1),
> which denies outbound network at the OS level. Proven by
> `tests/sandbox.rs::net_is_jailed_when_supported`: a real local TCP listener
> receives ZERO connections from the jailed plugin and the plugin's `net.connect`
> returns `EPERM`. `Session::net_jailed()` reports the truth so non-macOS callers
> (jail not yet wired — Linux seccomp / Windows AppContainer are the follow-ups)
> never over-claim. The original finding is kept below for context.

### Original finding (Node Permission Model does not restrict network)

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

## D3 — Plugin shares the host realm; frames are authenticated with a per-session nonce

**Date:** 2026-06-17, revised 2026-09-13 · **Constraints:** S1, TN1 · **Source:** m4 code-quality review, then a hostile-plugin security review

The plugin is loaded via `data:` import into the host's own Node process/realm, so
it shares `globalThis`/`process` with the host. The OS-enforced ambient denial
holds regardless, but the plugin can write to fd 1 and monkeypatch any global.
The original mitigation (stubbing `process.stdout.write`) was shown to be
bypassable: `Object.getPrototypeOf(process.stdout).write.call(...)`, a raw
`fs.writeSync(1, …)`, replacing the global `JSON.stringify`, or polluting
`Object.prototype.toJSON` all got a forged `{"t":"result"}` frame accepted by the
core ahead of the real one. `node:vm` is not a security boundary, so it is not
relied on.

**Design now (per-session nonce, `src-tauri/src/sandbox.rs` + `plugins/runtime/plugin-host.mjs`):**
- The core generates a 64-hex-char nonce (two OS-CSPRNG v4 UUIDs, 244 random
  bits) per session and sends it as the **first stdin line**, then waits for
  `ready`. It never sends the nonce again, nor puts it in argv/env.
- Before any plugin code exists in the realm, the host captures `fs.readSync`,
  `fs.writeSync`, `JSON.stringify/parse` and the few string primitives it needs,
  disables heap/trace introspection (`v8.getHeapSnapshot`, `writeHeapSnapshot`,
  `queryObjects`, `setFlagsFromString`, `trace_events.createTracing` →
  `ERR_ACCESS_DENIED`, synced into the ESM namespaces), then reads exactly the
  nonce line with the captured `readSync(0)` and keeps it only in module scope.
- Every frame is `"<nonce> " + JSON + "\n"`: built from a **null-prototype**
  object with the captured `stringify`, escaped to ASCII, and written with the
  captured `writeSync(1, string)`. The nonce only ever meets primitive string
  concatenation and a C++ binding, so no patchable prototype method sees it.
- The core's reader thread drops every stdout line that lacks the exact nonce
  (constant-time compare). `process.stdout` in the host resolves to stderr, so
  casual plugin prints go to logs rather than being dropped.
- **The core trusts only the frame's origin, never its content:** payloads are
  treated as plugin-controlled, and every security decision comes from the
  core's own broker. In particular `recheck` returns the broker's decision for
  the capability it brokered, not the host's `recheck_result` fields (proven
  with a deliberately lying host, `tests/fixtures/lying-host.mjs`).

Proven by `tests/sandbox.rs::forged_frames_are_rejected`, which runs six vectors
(stream-prototype write, raw fd-1 write, `JSON.stringify` replacement,
`Object.prototype.toJSON` pollution, V8 heap-snapshot scraping, and scanning
argv/env/stdin for a nonce-shaped string). It asserts the core returns the real
result every time, the heap snapshot is denied, and no nonce candidate is visible.

**How the plugin could still observe the nonce, and the status of each:**
argv/env (never there); stdin (consumed synchronously before the plugin loads;
the core never resends it); heap snapshot / `queryObjects` / V8 flags (disabled
by the host); `inspector`, `process.binding` (denied by `--permission`);
prototype patching of host primitives (captured before load; frames are
null-prototype).

**Residual risk:**
- **Stdin eavesdropping is accepted.** After the nonce line, the plugin can read
  what the core sends: its own source, the `invoke` args (`probe` only, never
  the focused resource, see D1), and the broker's replies to **its own**
  requests. One host process serves exactly one plugin, so it learns nothing
  it could not request itself. No core→host message carries a secret or any
  data the broker has not granted.
- **The shared realm remains.** A plugin can subvert host logic and steer
  *what* the host sends (for example, fake `cap_response`s to itself). This is
  bounded because the core derives every decision from its broker; the worst
  case is self-sabotage or a deadline kill.
- A future Node API, or a Node/V8 bug, that exposes heap contents would leak
  the nonce. The blast radius is that plugin's own session, and per the point
  above, forging frames there gains it nothing beyond what it can already do.
- Stronger isolation, with one process for the host and a separate process for
  the plugin, remains a follow-up if host↔core messages ever need to carry
  secrets.

## D4 — Plugin execution timeout + IPC line cap: ✅ DONE (build loop R1)

**Date:** 2026-06-17 · **Constraints:** S5, GAP-10 (resource exhaustion)

`Session` now reads on a dedicated reader thread into a **bounded**
`sync_channel`, and the reader caps each IPC line at `MAX_LINE_BYTES` (1 MiB)
via `Read::take`, so an unbounded line cannot exhaust memory. Every operation
(handshake, `load`, `invoke`, `recheck`) runs under one **overall** `Instant`
deadline (`Session::set_timeout`). The earlier per-message timeout let a plugin
that sent a request every 4.9 s run forever. A **watchdog** thread kills the
child at the deadline, which also unblocks a core pipe *write* that a plugin
stalled by not reading stdin. Any failed operation poisons the session: the
child is killed and **reaped at once**, with no zombie. `impl Drop for Session`
kills and reaps as well, so error paths never leak the child, and `shutdown`
waits only a short grace period before killing. Plugin stderr is piped,
forwarded up to 64 KiB, then drained and discarded. Proven by
`tests/sandbox.rs::{hung_plugin_is_killed, chatty_plugin_is_killed_at_overall_deadline,
flooding_plugin_cannot_block_core_writes, timed_out_child_is_reaped,
dropping_session_kills_and_reaps_child}`. NOTE: this is plugin-IPC DoS hardening; S5's
*own* scope (the public local link API size/rate bounds) is a separate later task.

## D5 — Integration facade: `UbilinkService` ties every round into the user flow (R6 / m6-integrate)

**Date:** 2026-06-17 · **Constraints:** B1, RT-1/RT-3/RT-4, T2, U5, S4, S5, RT-7/U3

`src-tauri/src/service.rs` is the single entry point a UI (or Tauri command layer)
calls. It composes the pieces built across R1–R5 into the core flow:
permission-gated capture (S4) → bounded-API re-validation of the adapter's URI,
defense-in-depth (S5/SC-1) → bidirectional link in the graph (RT-4) → backlinks
from either endpoint (U5), with honest degradation for unsupported apps (RT-7/U3,
non-empty `ServiceError::message`). Proven by 4 e2e tests in `service.rs`
(end-to-end capture→link→backlinks; content-hash survives validation; no-permission
degrades not panics; unknown-app capability explains). Full suite: **43 green**.

**No constraint flipped to SATISFIED.** B1, U5 and S4 keep their PARTIAL status:
their residuals (GUI panel, live macOS app capture, TCC permission grant) are
genuinely ENV-blocked in a headless environment and are escalated, not faked. This
round adds *integration evidence* — the headless slices now compose — not
satisfaction of the blocked work. `satisfied_count` stays 8.

## D6 — Live macOS capture: shrink the ENV-blocked surface with a CommandRunner seam (R7)

**Date:** 2026-06-17 · **Constraints:** B1 (live half), S4/RT-2, TN7, U3, T5

The live-app residual of B1 (capture a link to whatever is active in Safari /
Chrome / Finder) and the S4 automation-permission detection were wholly
ENV-blocked. `src-tauri/src/live_capture.rs` makes them real by applying a
functional-core / imperative-shell split: a `CommandRunner` trait is the single
I/O seam. The real `OsaScriptRunner` shells out to `osascript`; a fake runner lets
**10 unit tests** prove — headlessly — the AppleScript selection per app, the
`url\tTITLE` / Finder-path parsing into `ResourceIdentity`, U3 degradation
(empty / title-only / TCC-denied replies → explained `AdapterError`, never a
panic), the S4 gate firing *before* the runner is consulted, and the permission
probe mapping (`success → Granted`, `-1743 → Denied`, else `NotDetermined`).

TDD caught a real bug pre-commit: trimming the whole reply ate the leading tab and
masked an empty-URL field — fixed to trim line-ends only.

**ENV-blocked surface reduced, not eliminated.** What remains needs a real desktop:
one `osascript` seam against a live app under a TCC grant. It is covered by an
`#[ignore]`d live test (`live_capture_against_real_safari`) and
`docs/RUNBOOK-live-capture.md`. **No status flip** — B1/S4 stay PARTIAL until that
runbook is observed green on a Mac. `satisfied_count` stays 8. 53 tests green.
*(Superseded in part by D13: the per-app `AppKind` code and the
`live_capture_against_real_safari` test are gone; `docs/RUNBOOK-live-capture.md`
describes the current check.)*

## D7 — CI signed builds: a release-signing pipeline reusing the R3 crypto (R8 / O3)

**Date:** 2026-06-18 · **Constraints:** O3, S3/RT-10, O1/T4

O3 (release artifacts signed by a pipeline, not a dev laptop) was ENV-blocked. The
crypto is *not* new — `ubilink_core::release` reuses R3's `sign_bundle` +
`Installer::verify`, which already passed the GAP-09 attack matrix. O3 adds only the
operational seam: a JSON `WireBundle` wire format (hex-encoded sig/pubkey, dep-free
hex codec so S2 holds — no new crates), pure `sign_to_json` / `verify_json` entry
points keyed by hex (so the key comes from a CI secret), a thin `bin/ubilink-sign` CLI
(`sign` / `verify` / `pubkey`), and `.github/workflows/release.yml`
(test → sign → **self-verify** → upload; the build fails if the artifact doesn't
validate).

Proven headlessly: 7 unit tests (roundtrip, tamper→`BadSignature`,
untrusted→`UntrustedSigner`, malformed inputs) **plus** an end-to-end CLI smoke
against the real `plugins/examples/benign` bundle (trusted → exit 0, untrusted &
tampered → exit 1).

**No status flip.** O3 stays PARTIAL: its code/test cells are now ✓ but the ops cell
is ◐ — the residual is GitHub Actions actually running the workflow with the org
`UBILINK_SIGNING_KEY` secret (`docs/RUNBOOK-ci-signing.md`), which needs the real CI
environment. Escalated, not faked. `satisfied_count` stays 8. 60 tests green.

## D8 — GUI panel: no-build vanilla frontend + Rust command contract (R9 / U1·U2·U4·U3·U5)

**Date:** 2026-06-18 · **Constraints:** U1, U2, U3, U4, U5

**Stack choice, forced by the project's own constraints (not preference):** a
no-build, **zero-npm-dependency** vanilla frontend (`dist/`). A React/bundler SPA
would contradict S2 (third-party JS = potential egress) and add hydration latency
that fights U1's p99<200ms. The Rust core already refuses to depend on Tauri; the
panel keeps that line — its *logic* is plain Rust + plain JS, and the Tauri webview
binding is the only ENV-blocked layer.

The U1↔T5 tension (spec line 245) dictates the architecture: **render-first,
fill-async.** `dist/panel-state.mjs` is a pure view-state reducer where `open()`
transitions to `capturing` synchronously (no capture data in the first paint), and
`captured`/`failed` arrive later — proven by 8 `node --test` cases (built-in runner,
zero deps, S2-safe). `src-tauri/src/panel.rs` is the command contract
(`capture_current` over the integrated `UbilinkService`): it surfaces backlinks (U5)
and converts any failure to a non-empty explained message (U3) — 2 Rust tests. Three
more Rust tests assert the static **markup/style contract**: `index.html` has
`role="dialog"`, an accessible name, `aria-live`, `role="alert"`, a single primary
Copy action + Enter shortcut; `panel.css` has a `:focus-visible` ring and respects
`prefers-reduced-motion` (U4).

**U1/U2/U4 upgraded MISSING → PARTIAL** (the UI exists and is contract-tested), but
**no flip to SATISFIED.** The residuals genuinely need a real display: the p99/p50
timing, the VoiceOver + AA-contrast + keyboard-only pass, the global hotkey, and the
actual `#[tauri::command]` wiring — all in `docs/RUNBOOK-gui-panel.md`.
`satisfied_count` stays 8. 65 cargo + 8 node tests green.

## D9 — B3 extension surface SATISFIED + O2 capability matrix published (R10)

**Date:** 2026-06-19 · **Constraints:** B3 (→ SATISFIED), O2

**B3 — the first satisfied-count increase since R5 (8 → 9), and an honest one.** B3
asserts a *structural* property — "a documented extension surface lets anyone add an
integration without forking core" — not a runtime/live one, so it is fully provable
headlessly. The surface now exists end-to-end: `docs/AUTHORING-INTEGRATIONS.md` (the
walkthrough), `ubilink-sign validate` over `Manifest::validate` (author-facing check,
unit-tested), `ubilink-sign sign` (which now validates before signing), R3's
`Installer` (verify + trust + downgrade), and the R1 sandbox run. Adding an app
exercises these generic mechanisms and touches **zero** core files. No ENV residual —
unlike B1/U1/O3, B3 needs no real device to be true.

**O2 — published, drift-guarded; render is the only residual.** `docs/CAPABILITIES.md`
is GENERATED from `CapabilityMatrix::to_markdown()` and pinned to the code by the
`capabilities_doc_is_in_sync` test: the published disclosure can never silently
diverge from the matrix that enforces it. The in-app *data* path
(`PanelBackend::capabilities()`) is tested; the in-app *render* lives in the
ENV-blocked GUI (folded into `docs/RUNBOOK-gui-panel.md`), so O2 stays PARTIAL.

69 cargo + 8 node tests green.

## D10 — Integration registry as a separate std-only crate (R11 / O4)

**Date:** 2026-06-19 · **Constraints:** O4, B3 (ecosystem), S2, S3/RT-10

**Architecture call: the registry is a SEPARATE crate (`registry/`), not part of
ubilink-core.** A registry is a network *service*; S2 forbids the *core* from egress,
not a standalone service. Keeping it out of ubilink-core means the core's
`no_network_deps` guard stays physically true. The registry adds **zero** new
third-party deps — its HTTP layer is hand-rolled over `std::net` (no axum/hyper),
matching the project's minimalist supply-chain ethos — and it reuses ubilink-core's
proven ed25519 `verify_json` + `WireBundle` rather than reimplementing crypto.

Same functional-core / imperative-shell split: `RegistryIndex` (publish verifies +
enforces version monotonicity; get/list/search) and the pure `handle_request` router
are tested with no sockets; `handle_stream` (HTTP/1.1 parse) is tested with an
in-memory stream; `bin/ubilink-registry` is the thin `TcpListener` loop. Verified by 6
lib tests **and** a real-socket smoke: `POST /publish` → 201, list + `?q=` search,
verbatim bundle fetch, untrusted publisher → 400, unknown id → 404.

**Review = the trusted-key gate.** A publish is accepted only if signed by a key in
the registry's trusted set; approving a contribution means adding its publisher
pubkey, revoking means removing it — review is enforced cryptographically, not by
convention (`docs/RUNBOOK-registry.md`).

**Hardening (2026-09).** Trust in *a* key is not trust for *every* id: each id is
bound to the key that first published it, and a different trusted key cannot
publish over it (key rotation / id transfer is not supported yet). The manifest
schema is enforced (`Manifest::validate` + `deny_unknown_fields`) inside
`Installer::verify`, the one gate shared by publish, CI self-verify and install.
The HTTP shell reads each request with bounded sizes (1 MiB body → 413 before
allocating; 8 KiB request line; 16 KiB headers) and no lock held, and bounds
resources (10 s socket idle, 30 s request deadline, 64 connections → 503).

**O4 MISSING → PARTIAL, not SATISFIED.** The registry mechanism and the documented
review process exist and are proven; what remains is *operating a hosted, governed
instance* (public host, TLS, durable storage, a standing review team) — genuinely
ENV-blocked. Unlike B3 (a purely structural claim), O4 implies a running governed
channel, so it stays PARTIAL. `satisfied_count` stays 9. 75 cargo + 8 node tests green.

## D11 — Tauri desktop shell as a separate crate; T1 wiring closed in code (R12)

**Date:** 2026-06-19 · **Constraints:** T1, U1, U2, U3, U5, O2

**`desktop/` is a SEPARATE crate** (third time the rule applies, after registry/):
Tauri pulls a ~300-crate tree, and ubilink-core is deliberately Tauri-free (D1/D8) so
its S2 `no_network_deps` guard stays physically true. The desktop crate depends on
ubilink-core + `tauri` v2 + `tauri-plugin-global-shortcut`; the core is untouched.

It registers `#[tauri::command]` wrappers over the headlessly-tested `PanelBackend`:
`capture_current` (detects the frontmost app via `app_kind_for` → routes to the live
`MacAppAdapter` → capture+validate+backlinks, with an explained error for an
unsupported app, U3), `capabilities` (O2 disclosure), `copy_to_clipboard` (pbcopy,
no extra dep, U2), `open_link`, `dismiss_panel`. A Cmd/Ctrl+Shift+K global shortcut
toggles the hidden `panel` window (U1). `withGlobalTauri` exposes the bridge the
no-build `panel.js` already calls.

**Compilation is the verification.** The shell **compiles and links** against Tauri
v2 (`make build-desktop` → a 26 MB binary): the command signatures, the v2
global-shortcut API (`Shortcut::new`/`on`/`register`), and `generate_context!`
(config + capabilities + icon) all type-check. The frontmost-routing logic is
unit-tested in ubilink-core (`app_kind_for`). **`make run` now launches the real OS
app** (was the UI preview).

**T1's literal gap — "broker IPC not wired into Tauri commands" — is closed in
code.** It stays PARTIAL, not SATISFIED: there is no automated runtime test without a
display, so the trust-boundary's *runtime* enforcement is verified on a Mac
(`docs/RUNBOOK-gui-panel.md`). `satisfied_count` stays 9. 76 cargo + 8 node tests
green; desktop compiles.

## D12 — Inverted live-capture flow + S4 permission onboarding (R13)

**Date:** 2026-06-19 · **Constraints:** U1, U2, U3, S4/RT-2 · **Source:** running the app

Running the desktop app surfaced two real bugs that compilation could never catch:
(1) the panel **captured itself** — once shown it takes focus, so the frontmost app
is ubilink, not the app you were in; (2) capture ran **only once at page load**, so
re-summoning showed a stale result.

**Fix — invert the trigger.** The Rust global-shortcut handler now `capture_now()`s
the *currently* frontmost app (before the panel steals focus), `emit`s `capture-ok`
/`capture-err` to the webview, then shows the panel. `panel.js` became
event-driven: it listens for those events instead of capturing on load, so it links
the app you were in and re-captures on every press. Esc dismisses.

**S4 permission onboarding.** A launch screen guides the user through the two macOS
grants: Accessibility (`AXIsProcessTrusted` via a dependency-free
`#[link(name="ApplicationServices")]` FFI — the global hotkey needs it) and
Automation (the existing System Events probe). `permission_status` reports both;
`open_settings` jumps to the right privacy pane; a checklist shows ✓/✗ with a
Re-check. `permissionsComplete()` is a pure helper (node-tested); the onboarding
markup is Rust-tested; all of it compiles.

These advance U1/S4's *code* but **flip nothing** — the actual grant and the
on-screen behaviour are observed on a Mac. `satisfied_count` stays 9. 77 cargo + 9
node tests green; desktop compiles. (The "captures itself" class of bug is exactly
why U1–U5 were never flipped on compile alone — only a human running it finds them.)

## D13 — Adapters become declarative config, not code (R14)

**Date:** 2026-06-20 · **Constraints:** B3 (deepened), B1, T5 · **Source:** user — "can adapters be configuration instead of code, so we can add all the Linking Manifesto apps?"

The hardcoded `MacAppAdapter`/`AppKind`/`app_kind_for` enum was a stopgap — adding an
app meant editing Rust. Replaced with **declarative adapters** (`adapters_config.rs`):
an `AdapterSpec` is `{ app, script, fields, delimiter?, uri, name, path? }` where
`uri`/`name`/`path` are templates over the script's output fields, with `urlencode`
and `basename` transforms. `ConfiguredAdapter` runs the script via the existing
`CommandRunner` seam and renders the templates; `AdapterRegistry` loads embedded
defaults (`adapters.default.json`) and merges a user `adapters.json` (user entries
override built-ins by app name).

So **adding an app is a JSON entry, no recompile** — this extends B3 (already
SATISFIED for the plugin/broker path) down to the addressing layer itself, which is
the manifesto's whole premise: the ecosystem scales app coverage. Seeds 10 apps
(Safari, Chrome, Arc, Brave, Edge, Vivaldi, Opera, Finder, Bear, Things3); the
~80-app list of linking-capable apps
is now reachable as config. Honest scope: only seeds with verifiable patterns ship;
the rest need a per-app script + a Mac with the app (`docs/SUPPORTED-APPS.md`) — but
that work is now local config a user can add, not a core change. *(Amended by D14:
adapter JSON is trusted local configuration running with the user's privileges, not
a sandboxed plugin; distributing community adapter JSON is **not** supported.)*

The template engine (`render`) and `parse` are pure and unit-tested, including a
guard that **every shipped spec's templates render** (a typo can't ship a broken
adapter). The single `osascript` call is the only ENV-blocked part. 77 cargo + 9
node tests green; desktop compiles.

## D14 — Adapter JSON is trusted local configuration, not a plugin

**Date:** 2026-09-13 · **Constraints:** S1, S2, S4, B3, TN1 · **Source:** security review of the desktop plugin manager

**Decision.** A declarative adapter (`adapters.default.json` embedded, plus the
user's `adapters.json` merged by `AdapterRegistry::extend_from_json`) is **trusted
local configuration**, equivalent to a script the user wrote and runs as
themselves. It is **not** a plugin and is **not** sandboxed. Distributing
community adapters (copy-paste JSON, gists, a shared list) is **not supported**.

**Why.** An adapter's `script` runs via `osascript -e` in the Ubilink process's
context: the user's full privileges plus whatever macOS Automation/Accessibility
(TCC) grants Ubilink holds. AppleScript's `do shell script` makes that arbitrary
code execution. None of the D1–D4 guarantees (`node --permission`, the
`sandbox-exec` net jail, the capability broker) apply, because adapters never go
through the Node plugin runtime. D13 blurred this by inviting community-contributed
adapter JSON, and the README's "Security model" section implied the plugin sandbox
covered all extensions. Calling user JSON "third-party plugins" in the UI made it
worse: a user would reasonably assume it was contained.

**Applied now.**
- `builtin` is honoured only for the embedded defaults. `extend_from_json` forces
  `builtin = false` on every user entry, including one that sets
  `"builtin": true` or overrides a built-in by app name. Overriding a built-in
  stays allowed (users may fix a broken script), but the result is a user-defined,
  disableable adapter. A new `AdapterSpec::user_defined` flag (never read from JSON)
  marks these entries. `AdapterRegistry::from_json`, the path that honours
  `builtin`, is now private to the embedded defaults. Tests in `adapters_config.rs`
  cover this.
- The plugin manager labels user entries "Custom adapter · runs with your
  permissions" instead of "Third-party". README (Security model) and
  `docs/SUPPORTED-APPS.md` say plainly that custom adapters are unsandboxed.

**What community distribution would need (not built).** Community adapters must
ship as **signed bundles** through the existing S2/S3 path: `ubilink-sign`,
`Installer::verify` with trust and downgrade checks, and the O4 registry's
trusted-key review. They must also execute **behind the capability broker**
instead of raw `osascript -e`. For example, the broker could expose a restricted,
core-owned set of AppleScript *templates* ("front document URL of app X",
"selected item id of app X"), where a bundle supplies only parameters and an app
name gated by a declared capability, so no bundle can supply free-form script text
or `do shell script`. Until that exists, adapter JSON is local-only.

## D15 — An integration signature covers the full manifest

**Date:** 2026-09-13 · **Constraints:** S3/RT-10, RT-6 · **Source:** security review (PR #1)

The bundle signature used to cover only `id`, `version` and `source`, but the broker
turns `manifest.needs` directly into grants. Anyone who could edit a signed bundle
(registry, mirror, cache) could widen its capabilities and it still verified. The
newline-joined encoding was not injective either.

`integrations::canonical_message` now signs every manifest field (`id`, `name`,
`app`, `version`, each of `needs`), the bundle's integer version and the source,
each length-prefixed, under the domain tag `ubilink-integration/v2`. An exhaustive
destructure of `Manifest` means a new field fails to compile until it is signed or
explicitly excluded. Bundles signed in the old format no longer verify; none were
committed, and CI re-signs on every run.

## D16 — One link validation policy for capture, import and open

**Date:** 2026-09-13 · **Constraints:** S5/SC-1, U3 · **Source:** security review (PRs #3, #4, #11)

A link crosses three boundaries: capture, import (a shared export file) and open
(handing it to the OS). Each had its own checks, or none. Now:

- **One validator.** `LinkPolicy::validate(uri, name)` (`src-tauri/src/api.rs`) is
  used by capture (`LinkApi`), import (`UbilinkService::import`) and open
  (`adapter::open_uri`, which re-validates stored rows as defence in depth). It checks
  size caps, control and bidi-override characters, a leading `-`, scheme syntax, the
  deny-list, then the allowlist.
- **Hard deny-list.** `DENIED_SCHEMES` (`javascript`, `vbscript`, `livescript`,
  `data`, `blob`, `view-source`, `jar`, `ms-msdt`, `search-ms`, `search`,
  `ms-officecmd`, `ms-appinstaller`) is checked before the allowlist, so schemes that
  adapters declare in user-editable `adapters.json` can never re-admit them.
- **`file:` stays, but local only.** Finder's built-in adapter emits `file://` links,
  so `file` remains allowed. The link must name this computer: an empty host or
  `localhost` (any case), and no UNC path (`file:////server/…`, `file:\\server\…`,
  percent-encoded forms). Anything else is refused with `ApiError::RemoteFile`,
  because opening it on Windows contacts that host over SMB.
- **Import is all-or-nothing.** `LinkGraph::import_json` validates every row it
  would write before opening the write transaction. One bad row rejects the whole
  file, leaves the graph unchanged, and the error names the row.
- **Openers never use a shell.** `adapter::open_plan_for` runs `open -- <uri>` on
  macOS, `rundll32 url.dll,FileProtocolHandler <uri>` on Windows (no `cmd.exe`;
  space and `"` percent-encoded) and `xdg-open <uri>` on Linux (which rejects `--`;
  the leading-`-` check covers it). `ConfiguredAdapter::open_plan` uses the same
  function.
- **Launchable targets are revealed, not run.** A `file://` link to something that
  would run (an app bundle, script, installer or shortcut, judged by extension after
  percent-decoding, or on Unix a file with an execute bit, following symlinks) is
  shown instead: `open -R --` selects it in Finder, and Windows/Linux open the
  containing folder.

## D17 — Bounded capture: a 15 s `osascript` deadline, and no lock across it

**Date:** 2026-09-13 · **Constraints:** T4, U3 · **Source:** reliability review (PR #7)

A stalled target app used to hang capture until the ~120 s Apple Event timeout, and
the desktop held the backend lock the whole time, so one stuck app froze every other
command.

- **Deadline.** Every `osascript` call goes through `OsaScriptRunner` →
  `live_capture::run_bounded`: both pipes are drained on their own threads, the
  child is killed and reaped at one overall deadline, and the error explains what
  to do. `OSASCRIPT_TIMEOUT` is **15 s**, not shorter, because the first capture
  from an app can raise the macOS Automation (TCC) prompt and `osascript` blocks
  until the user answers it. A normal capture answers in well under a second.
- **No lock across the adapter call.** `PanelBackend::capture_current` takes the
  shared `&Mutex<PanelBackend>` and locks only for the fast steps. It reads the
  permission (S4), releases the lock, runs the adapter, then re-locks to validate
  the link (D16), store it and read its backlinks. Proven by
  `panel.rs::{backend_serves_other_commands_while_a_capture_is_in_flight,
  a_captured_link_is_still_validated_before_it_is_stored}`.

## D18 — Versioned, atomic graph migrations

**Date:** 2026-09-13 · **Constraints:** RT-4, T2 · **Source:** reliability review (PR #8)

The old resources-table migration ran several `ALTER`s outside a transaction and was
gated only on the presence of one column, so a crash midway left a DB that the
check skipped forever and that failed on the next open.

- **Versioned.** The schema version lives in `PRAGMA user_version`. `MIGRATIONS` in
  `src-tauri/src/graph.rs` is **append-only**: `MIGRATIONS[i]` takes version `i` to
  `i + 1`, and a shipped step is never edited or reordered.
- **Atomic.** Each step runs in one `IMMEDIATE` transaction that also bumps
  `user_version`, re-reading the version inside it. A failed or interrupted step
  rolls back completely and is retried on the next open, and two processes can't
  both apply it. An up-to-date DB takes no write lock on open.
- **Legacy DBs converge.** Unversioned (v0) DBs, including half-migrated ones,
  reach the same final schema because the steps inspect the actual columns.
- **Related invariants.** `updated_at` never moves backwards: upserts keep the
  larger of the stored and incoming time, so linking with no time doesn't sink a
  resource in the newest-first list. `link(a, a)` is a no-op that writes nothing.

## D19 — Registry ids are bound to their first signer; the schema gate is shared

**Date:** 2026-09-13 · **Constraints:** O4, S3/RT-10, RT-6 · **Source:** security review (PR #9)

D10's hardening paragraph records the mechanism. This decision records its limits.

- Each id is owned by the key that first published it (`owner_hex`). A bundle for
  that id signed by any other key, even a trusted one, is refused.
- **No rotation yet.** An owner who loses or rotates their key cannot publish that
  id again without operator intervention.
- **Ownership is not persisted.** The index is in memory, so a restart forgets every
  owner, and the first trusted publish after it claims the id. A hosted instance
  must persist owners alongside bundles (`docs/RUNBOOK-registry.md`).
- The manifest schema is enforced in `Installer::verify` (`Manifest::validate` plus
  `#[serde(deny_unknown_fields)]`, i.e. `additionalProperties: false`). That is the
  one gate shared by on-device install, CI self-verify (`release::verify_json`) and
  registry publish. Because the signature covers only known fields (D15), an unknown
  field would otherwise be a smuggling channel.
- HTTP bounds: 1 MiB body (413 before allocating), 8 KiB request line, 16 KiB
  headers, 10 s socket waits, 30 s per request, 64 connections (503 over the limit).

## D20 — The capability matrix is derived from the adapter registry

**Date:** 2026-09-13 · **Constraints:** O2, RT-7, U3 · **Source:** review (PR #11)

The hand-written matrix claimed Obsidian, which has no adapter, and omitted most of
the adapters that ship, so the published disclosure both over- and under-claimed.

`CapabilityMatrix::from_registry` now builds the rows from the adapters that exist:
one macOS/AppleScript row per adapter (they all capture through `osascript`), plus
the `Files` path fallback on every platform. `baseline()` is the matrix of the
built-in registry. `docs/CAPABILITIES.md` is generated from it and pinned by
`capabilities_doc_is_in_sync_with_the_matrix`. The desktop passes its merged registry
(defaults + `adapters.json`) through `UbilinkService::with_capabilities`, so the
`capabilities` command reports user adapters too. Off macOS, `capability()` explains that the
adapter needs macOS AppleScript and offers the file-path fallback.
