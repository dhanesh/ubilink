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

## D2 — Network containment: ✅ CLOSED via macOS sandbox-exec net jail

> **Resolved 2026-06-17 (build loop R1).** The host is now spawned wrapped in
> `sandbox-exec -p '(version 1)(allow default)(deny network*)'` on macOS, which
> denies outbound network at the OS level. Proven by
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

## D4 — Plugin execution timeout + IPC line cap: ✅ DONE (build loop R1)

**Date:** 2026-06-17 · **Constraints:** S5, GAP-10 (resource exhaustion)

`Session` now reads on a dedicated reader thread; the main thread reads with
`recv_timeout` and **kills the child** on timeout (`Session::set_timeout`), and the
reader caps each IPC line at `MAX_LINE_BYTES` (1 MiB) via `Read::take` so an
unbounded line cannot exhaust memory. Proven by
`tests/sandbox.rs::hung_plugin_is_killed` (a never-returning plugin is killed in
<1s instead of blocking forever). NOTE: this is plugin-IPC DoS hardening; S5's
*own* scope (the public local link API size/rate bounds) is a separate later task.

## D5 — Integration facade: `HookyService` ties every round into the user flow (R6 / m6-integrate)

**Date:** 2026-06-17 · **Constraints:** B1, RT-1/RT-3/RT-4, T2, U5, S4, S5, RT-7/U3

`src-tauri/src/service.rs` is the single entry point a UI (or Tauri command layer)
calls. It composes the pieces built across R1–R5 into the core flow:
permission-gated capture (S4) → bounded-API re-validation of the adapter's URI,
defense-in-depth (S5/SC-1) → bidirectional hook in the graph (RT-4) → backlinks
from either endpoint (U5), with honest degradation for unsupported apps (RT-7/U3,
non-empty `ServiceError::message`). Proven by 4 e2e tests in `service.rs`
(end-to-end capture→hook→backlinks; content-hash survives validation; no-permission
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

## D7 — CI signed builds: a release-signing pipeline reusing the R3 crypto (R8 / O3)

**Date:** 2026-06-18 · **Constraints:** O3, S3/RT-10, O1/T4

O3 (release artifacts signed by a pipeline, not a dev laptop) was ENV-blocked. The
crypto is *not* new — `hooky_core::release` reuses R3's `sign_bundle` +
`Installer::verify`, which already passed the GAP-09 attack matrix. O3 adds only the
operational seam: a JSON `WireBundle` wire format (hex-encoded sig/pubkey, dep-free
hex codec so S2 holds — no new crates), pure `sign_to_json` / `verify_json` entry
points keyed by hex (so the key comes from a CI secret), a thin `bin/hooky-sign` CLI
(`sign` / `verify` / `pubkey`), and `.github/workflows/release.yml`
(test → sign → **self-verify** → upload; the build fails if the artifact doesn't
validate).

Proven headlessly: 7 unit tests (roundtrip, tamper→`BadSignature`,
untrusted→`UntrustedSigner`, malformed inputs) **plus** an end-to-end CLI smoke
against the real `plugins/examples/benign` bundle (trusted → exit 0, untrusted &
tampered → exit 1).

**No status flip.** O3 stays PARTIAL: its code/test cells are now ✓ but the ops cell
is ◐ — the residual is GitHub Actions actually running the workflow with the org
`HOOKY_SIGNING_KEY` secret (`docs/RUNBOOK-ci-signing.md`), which needs the real CI
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
(`capture_current` over the integrated `HookyService`): it surfaces backlinks (U5)
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
walkthrough), `hooky-sign validate` over `Manifest::validate` (author-facing check,
unit-tested), `hooky-sign sign` (which now validates before signing), R3's
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
hooky-core.** A registry is a network *service*; S2 forbids the *core* from egress,
not a standalone service. Keeping it out of hooky-core means the core's
`no_network_deps` guard stays physically true. The registry adds **zero** new
third-party deps — its HTTP layer is hand-rolled over `std::net` (no axum/hyper),
matching the project's minimalist supply-chain ethos — and it reuses hooky-core's
proven ed25519 `verify_json` + `WireBundle` rather than reimplementing crypto.

Same functional-core / imperative-shell split: `RegistryIndex` (publish verifies +
enforces version monotonicity; get/list/search) and the pure `handle_request` router
are tested with no sockets; `handle_stream` (HTTP/1.1 parse) is tested with an
in-memory stream; `bin/hooky-registry` is the thin `TcpListener` loop. Verified by 6
lib tests **and** a real-socket smoke: `POST /publish` → 201, list + `?q=` search,
verbatim bundle fetch, untrusted publisher → 400, unknown id → 404.

**Review = the trusted-key gate.** A publish is accepted only if signed by a key in
the registry's trusted set; approving a contribution means adding its publisher
pubkey, revoking means removing it — review is enforced cryptographically, not by
convention (`docs/RUNBOOK-registry.md`).

**O4 MISSING → PARTIAL, not SATISFIED.** The registry mechanism and the documented
review process exist and are proven; what remains is *operating a hosted, governed
instance* (public host, TLS, durable storage, a standing review team) — genuinely
ENV-blocked. Unlike B3 (a purely structural claim), O4 implies a running governed
channel, so it stays PARTIAL. `satisfied_count` stays 9. 75 cargo + 8 node tests green.
