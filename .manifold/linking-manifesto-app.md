# linking-manifesto-app

## Outcome

Create a cross-platform desktop application that perfectly implements the
[Linking Manifesto](https://linkingmanifesto.org/), is open source, and is
extensible so other users can add capabilities and extend it with integrations.

**What "perfectly implements the manifesto" means (discovered in m1):** The manifesto
(ed. Luc P. Beaudoin) is implementation-silent — it specifies no formats or protocols.
Its concrete demand is two behaviors, which this app must realize *universally* across
the resources a user works with:

1. **UI** — let users conveniently obtain a link to the currently open or selected resource.
2. **API** — provide an API to obtain or construct a link to that resource (its address and name).

`ubilink`
is an open-source, cross-platform, extensible realization of the same idea: a hotkey-invoked
tool that captures a robust link to whatever resource is active in *any* app, stores a
**bidirectional** graph of links between resources, lets users navigate that graph, and is
extended per-app by community integrations.

**Architectural decisions locked in m1:**
- **Framework:** Tauri (Rust core for OS automation + the link graph; web UI; capability-gated IPC).
  **Desktop shell superseded 2026-10-04 (D22):** the UI panel was the Tauri webview
  (D8/D11); it is now a native GPUI app (`desktop-gpui/`, building `Ubilink.app`)
  — no second WebKit process, no bundler step. Since the Phase 2b cutover (2026-10-10,
  D24) it is the only app: the Tauri shell (`desktop/`) and its web panel (`dist/`) are
  deleted. Command logic lives in a GUI-free `app/` crate with no `tauri` or `gpui`
  dependency, so the Rust core/broker architecture (T1) is otherwise unchanged. See
  D22/D23/D24.
- **Extensibility:** third-party integrations = declarative manifest + sandboxed JS, distributed as installable bundles.
- **Platform scope:** macOS-first with deep fidelity in v1; core is platform-agnostic so Windows/Linux are adapters, not rewrites.
- **License:** MIT.
- **Data/privacy:** local-only link graph (SQLite), zero telemetry; sync is a future opt-in plugin.
- **Link model:** bidirectional graph (backlinks) in v1.
- **Link resilience:** best-effort multi-identifier re-resolution.

---

## Constraints

### Business

#### B1: Faithfully realizes the manifesto's two core asks
The app MUST (a) provide a UI to obtain a link to the currently open/selected resource, and
(b) expose an API to obtain or construct a link (address + name) to that resource. This is the
literal, non-negotiable definition of "perfectly implements the manifesto".
> **Rationale:** The manifesto's only concrete demands. Everything else is a means to these ends.
> **Type:** invariant · **Source:** interview · **Challenger:** stakeholder

#### B2: Open source under the MIT license
The project is released under MIT — permissive, commercial-plugin-friendly, fork-friendly.
> **Rationale:** "Ubiquitous" linking wants maximum adoption and zero-friction extension; MIT removes legal barriers for integrators and forks.
> **Type:** invariant · **Source:** interview · **Challenger:** stakeholder

#### B3: Third parties can add integrations without forking core
A documented extension surface lets anyone add support for a new app via an installable
manifest+JS bundle — no changes to, or rebuild of, the core application.
> **Rationale:** "Extensible for other users to add capabilities and integrations" is a primary outcome; the ecosystem, not the core team, must scale app coverage.
> **Type:** goal · **Source:** interview · **Challenger:** stakeholder

#### B4: macOS-first v1 with a credible cross-platform path
v1 ships deep, faithful linking on macOS (where the automation surface and the manifesto's
originators live); the core is platform-agnostic so Windows/Linux follow as adapters.
> **Rationale:** Attempting equal depth on all three at once forces a lowest-common-denominator (files/web only) result. Sequencing protects fidelity without abandoning the cross-platform promise.
> **Type:** boundary · **Source:** interview · **Challenger:** stakeholder

### Technical

#### T1: Tauri architecture with capability-gated IPC
Rust core (OS automation + link graph) and a web UI, communicating over capability-gated IPC.
Plugins run in a sandbox (WASM/JS) — never in the Rust core's trust domain.
> **Rationale:** Small footprint, a real security boundary, and a first-class plugin model — the best fit for "secure + extensible".
> **Type:** boundary · **Source:** interview · **Challenger:** stakeholder
> **Shell evolution (D22, 2026-10-04):** "web UI" here was the Tauri webview (D8/D11).
> It is superseded by a native GPUI shell (`desktop-gpui/`) that has no webview and no
> core↔UI IPC boundary at all — it calls the same tested command logic in-process via
> the GUI-free `app/` crate. T1's capability-gated-IPC property (core ↔ plugins via the
> broker, S1/RT-5) is unchanged; only the removed layer is core ↔ UI webview IPC.

#### T2: Bidirectional link graph in a local store
Links are stored as a persistent, bidirectional graph (SQLite). Hooking A↔B is atomic and
consistent; opening either endpoint surfaces the other as a backlink. Writes are transactional
and thread-safe under concurrent captures.
> **Rationale:** A two-way persistent graph is what makes this a linking *tool* rather than a clipboard helper. Concurrency-safety (GAP-17) because captures can race.
> **Type:** invariant · **Source:** interview · **Challenger:** technical-reality

#### T3: Best-effort link re-resolution via multiple identifiers
Each resource link stores several identifiers (path + OS security-scoped bookmark + content
hash + name). When the primary identifier breaks (moved/renamed), the system heuristically
re-resolves. **Target:** re-resolve ≥ 95% of links after an intra-volume move/rename.
> **Rationale:** Broken links erode trust in a linking tool; absolute permanence is impossible, so a measurable best-effort goal is the honest contract.
> **Type:** goal · **Source:** interview · **Challenger:** technical-reality · **Threshold:** statistical, resolve_rate ≥ 95% (intra-volume move/rename)

#### T4: Integrations are versioned, health-checked, and updatable independent of core
Each integration declares a version and is updated separately from the core app. The system
health-checks integrations (does the target app's URL scheme / automation API still respond?),
isolates faults to a single integration, and applies timeouts to external calls.
> **Rationale:** [Pre-mortem, external] A third-party app changing/removing its URL scheme must not silently break links or require a core release to fix. (GAP-11)
> **Type:** invariant · **Source:** pre-mortem · **Challenger:** technical-reality

#### T5: Resource-addressing behind a platform/app adapter interface
All "get a link to / open" logic sits behind an adapter trait (per platform, per app). macOS
adapters (AppleScript / Accessibility / URL schemes) ship in v1; Windows/Linux are additional
adapter implementations, not a rewrite. This is the core data path (GAP-03):
active resource → capture (adapter, crosses process/OS boundary) → store (graph) →
resolve → open (adapter, crosses boundary). Each boundary transition needs an integration test
and a defined fallback.
> **Rationale:** Keeps the cross-platform promise honest and makes "add an app" / "add a platform" a bounded, testable unit of work.
> **Type:** boundary · **Source:** interview · **Challenger:** technical-reality

### User Experience

#### U1: Panel invocation feels instant
Global hotkey → linking panel visible. **Target:** p99 < 200ms, p50 < 80ms (hotkey-to-visible).
> **Rationale:** The manifesto's premise is flow preservation; a laggy panel *is* the context-switch it's meant to eliminate.
> **Type:** boundary · **Source:** interview · **Challenger:** stakeholder · **Threshold:** statistical, p99 200ms / p50 80ms
> **Evidence (GPUI Phase 0, D22 — RT-9):** `desktop-gpui/probes/panel-hotkey.json` —
> shell hotkey-to-first-frame on stub views, debug automation build, 50 triggers:
> p50 ≈ 19–21ms / p99 ≈ 25–66ms, inside budget with margin (VERIFIED, scoped to the
> shell only). The full-view measurement on the release build is Phase 1's R15
> make-budget probe (PENDING). Supersedes reliance on the Tauri panel's own
> measurements (D8/D11 → D22).

#### U2: Copy-link-to-current-resource in ≤ 2 user actions
From a standing start (resource focused in another app), obtaining its link takes at most two
user actions (e.g., hotkey + confirm/copy).
> **Rationale:** Friction here defeats the purpose; copying a link must rival copying text in effort.
> **Type:** goal · **Source:** interview · **Challenger:** stakeholder · **Threshold:** deterministic, ≤ 2 user actions
> **Evidence (GPUI Phase 1, planned — RT-9):** `desktop-gpui/probes/copy-link.json`
> (R2/U2) — Enter or Cmd-C in the captured state copies the link in exactly two
> actions from the hotkey; not yet landed (PENDING).

#### U3: Graceful, explained degradation — never silent failure
When a resource or app cannot be linked (no adapter, permission missing, scheme unsupported),
the UI explains *why* and what is possible instead — it never fails silently or pretends.
> **Rationale:** [Pre-mortem, foreseeable] Off-macOS or for unsupported apps the tool must feel honest, not broken. Trust depends on transparency about limits.
> **Type:** invariant · **Source:** validated-pre-mortem · **Challenger:** stakeholder
> **Evidence (RT-7):** the capability-matrix degradation data path (O2) is SATISFIED
> and shell-agnostic, unaffected by the shell swap (D22). The GPUI panel's own live
> explained-failure render is `desktop-gpui/probes/capture-view.json` (R1/U3) — not
> yet landed (PENDING).

#### U4: Keyboard-first and accessible
Core flows are fully keyboard-operable; UI elements carry screen-reader labels and meet
reasonable contrast/focus standards.
> **Rationale:** A productivity tool invoked dozens of times a day must be fast for keyboard users and usable with assistive tech.
> **Type:** goal · **Source:** interview · **Challenger:** stakeholder
> **Evidence (GPUI Phase 0, D22/D23 — RT-11, new):** `desktop-gpui/probes/a11y-spike.json`
> (R7/TN5) — AccessKit roles + text for List/Input/Switch/Dialog, and keyboard-only
> Tab/Escape/arrow-key navigation, verified on stub views (VERIFIED, partial). Live
> VoiceOver parity is explicitly deferred to an attended Phase 2 check (PENDING, D22).

#### U5: Backlinks are visible at the resource
Opening or focusing a linked resource surfaces what is hooked to it (its backlinks), enabling
graph navigation, not just one-shot link copying.
> **Rationale:** Realizes the bidirectional model's value; "connected information" must be visibly connected from either end.
> **Type:** goal · **Source:** interview · **Challenger:** stakeholder
> **Evidence (GPUI Phase 1, planned — RT-12, new):** `desktop-gpui/probes/backlinks.json`
> (R3/U5) — newest-first backlinks rendered in the panel; not yet landed (PENDING).
> The bidirectional data layer itself is proven separately (RT-4).

### Security

#### S1: Plugin sandbox is deny-by-default
Third-party integration code (sandboxed JS) has no ambient network or filesystem access.
Capabilities are explicitly granted; a plugin can only see the resource it is invoked for —
never the link graph wholesale. Plugins never execute in the Rust core's trust domain.
> **Rationale:** [Pre-mortem, surprising] A malicious/buggy integration exfiltrating the graph or running arbitrary commands would destroy the zero-telemetry privacy promise overnight.
> **Type:** invariant · **Source:** validated-pre-mortem · **Challenger:** technical-reality

#### S2: Local-only data, zero telemetry
The link graph and all metadata stay on-device. No analytics, no background phone-home.
Network access happens only when a plugin explicitly requests it with user consent (per S1).
> **Rationale:** A linking tool indexes references to everything a user touches — an acute privacy surface. Local-only is the strongest, most verifiable guarantee.
> **Type:** invariant · **Source:** interview · **Challenger:** stakeholder

#### S3: Integrations are signed/verified before install
Installable integration bundles carry a verifiable signature; the app refuses or clearly warns
on unsigned/forged/downgraded bundles.
> **Rationale:** Provenance for community code is the first line of defense around the sandbox. (GAP-09 attack matrix: forged signature, unsigned acceptance, downgrade.)
> **Type:** boundary · **Source:** interview · **Challenger:** stakeholder

#### S4: Least-privilege OS automation with transparent rationale
macOS Accessibility/Automation (and platform equivalents) permissions are requested only when
needed, scoped minimally, with a plain-language reason; the app detects and surfaces missing
permission state.
> **Rationale:** Deep linking requires powerful OS automation rights; requesting them opaquely or over-broadly is both a security and a trust failure.
> **Type:** invariant · **Source:** interview · **Challenger:** regulation

#### S5: The local link API is bounded and rate-limited
The API mandated by B1 (manifesto ask #2) validates and bounds input: scheme allowlist,
payload size cap (≤ 64KB), encoding normalization, and rate limiting.
> **Rationale:** An unauthenticated local endpoint is a resource-exhaustion and injection surface (GAP-10/GAP-14). Bounds prevent a local process from abusing it.
> **Type:** boundary · **Source:** interview · **Challenger:** technical-reality · **Threshold:** deterministic, input ≤ 64KB + rate-limited

### Operational

#### O1: Integrations install/update independently of core releases
Users can install, update, and remove integrations without upgrading or rebuilding the core
app; a broken integration can be fixed by shipping a new integration, not a new app version.
> **Rationale:** [Pre-mortem, external] Decoupling integration lifecycle from core release is what makes T4's resilience operationally real.
> **Type:** goal · **Source:** pre-mortem · **Challenger:** stakeholder

#### O2: Per-platform / per-app capability matrix is published and surfaced in-app
A capability descriptor states what linking works where (which apps, which platforms, via which
mechanism); it is published in docs and shown in-app to set expectations.
> **Rationale:** [Pre-mortem, foreseeable] Honest, discoverable capability disclosure prevents the "macOS-only in disguise" failure and drives U3's degradation messaging.
> **Type:** goal · **Source:** validated-pre-mortem · **Challenger:** stakeholder

#### O3: Reproducible, signed cross-platform builds
Releases are reproducible and code-signed per platform (notarized on macOS), produced by a
documented CI pipeline.
> **Rationale:** A tool requesting deep OS automation permissions must be verifiably the build it claims to be; signing/notarization is table stakes for install trust.
> **Type:** goal · **Source:** interview · **Challenger:** stakeholder

#### O4: Documented community contribution & integration-registry process
There is a documented path to contribute code and to publish/review integrations (a registry
with review), enabling the ecosystem promised by B3.
> **Rationale:** "Extensible for other users" only materializes if there's a real, governed channel for outside contributions and discoverable integrations.
> **Type:** goal · **Source:** interview · **Challenger:** stakeholder

---

## Suggested Constraints (staged — not yet counted)

### SC-1: Boundary input validation (from GAP-14)
Validate the local link API and link-import boundary: URI-scheme allowlist, payload size cap,
encoding normalization, and schema-validation of resource identifiers. Promote into S-category
when confirmed.

### SC-2: Re-resolution confirm UX (from TN5 / T3)
When a moved/renamed resource is matched only by fuzzy heuristics, show "now points to X (moved
from Y) — confirm?" before retargeting. Auto-resolve silently only on exact content-hash/bookmark.

### SC-3: Revocable capability grants (from TN1 / S1, GAP-16)
Per-integration capability grants are revocable and force-invalidatable: revocation takes effect
immediately for in-flight and future broker calls.

### SC-4: Prefetch-link cache invalidation (from TN3 / U1, GAP-16)
A prefetched active-resource link is invalidated on focus change or resource mutation, so the
panel never presents a stale link.

---

## Tensions

All 7 surfaced tensions are **resolved**. No invariant was traded down — every INVARIANT-vs-GOAL
conflict was resolved by *transforming the goal to live inside the invariant's box* (TRIZ).

### TN1: Extensibility vs Sandbox containment
`B3` (extensible without forking core, GOAL) competes with `S1` (deny-by-default plugin sandbox,
INVARIANT). Plugins must be powerful enough to reach into other apps, yet contained.
*Physical contradiction — Privacy vs Usability — TRIZ P1/P24/P10/P15.*
> **Resolution:** (Transform) A **capability broker**. Plugins declare needed capabilities in their
> manifest (P10 prior action); the user grants them per-integration (P15); every OS/graph access is
> mediated by a broker that enforces the granted scope (P24 intermediary). A plugin gets exactly
> what it declared and nothing ambient; the link graph is never exposed wholesale. S1 stays intact,
> B3 is satisfied *inside* the sandbox. Spawns SC-3 (revocable/force-invalidatable grants, GAP-16).
> **Propagation:** T1 TIGHTENED (IPC must broker every plugin call); B3 LOOSENED (concrete extension surface).

### TN2: Universal fidelity vs macOS-first scope
`B1` (faithful/universal, INVARIANT) competes with `B4` (macOS-first, BOUNDARY).
*Global vs Local optimum — TRIZ P3/P1/P17.*
> **Resolution:** (Partition) "Universal" is scoped to the **supported set**: B1 holds iff, for every
> supported adapter, both the UI and API linking behaviors work (P3 local quality). The supported set
> is made explicit by the O2 capability matrix and grows over time; B4 merely sequences which adapters
> ship first. The invariant is asserted per-adapter, not over all apps on earth.
> **Propagation:** O2 TIGHTENED (matrix now defines B1's scope); B1 LOOSENED (scoped, still invariant within scope).

### TN3: Instant panel vs deep-capture latency
`U1` (panel p99<200ms, BOUNDARY) competes with `T5` (automation capture can cost 100s of ms).
*Technical contradiction — Speed vs Correctness — TRIZ P10/P11/P24.*
> **Resolution:** (Transform) Split panel-display from link-capture. A local focus-change watcher
> **prefetches** the active resource's link on idle (P10), so it's usually ready on open. The panel
> always appears instantly (U1 = hotkey-to-visible); if the link isn't ready it shows with a spinner
> and fills async (P11 cushioning). Spawns SC-4 (prefetch cache invalidation, GAP-16).
> **Failure cascade (GAP-06):** prefetch miss → async capture; async capture fails → **U3 explained
> degradation** ("couldn't get a link — here's why"). Terminates at an explicit accept-loss endpoint.
> **Propagation:** T5 LOOSENED (capture off critical path); U1 TIGHTENED (adds local prefetch watcher).

### TN4: Zero telemetry vs debuggability
`S2` (local-only, zero telemetry, INVARIANT) competes with `O1`/`O4` (diagnose & improve integrations).
*Physical contradiction — Privacy vs Usability — TRIZ P1/P24/P10.*
> **Resolution:** (Transform) Integration errors are logged **locally** (tied to T4 health-check). For a
> bug report, the user explicitly exports a **reviewable** diagnostic bundle (P1 segmentation: background
> telemetry vs user-initiated report; P24 intermediary). Registry quality comes from public reviews/issues,
> not usage data. Nothing leaves the device without an explicit user action — S2 fully preserved.
> **Propagation:** T4 TIGHTENED (must emit structured local logs); S2 unchanged (no compromise).

### TN5: Best-effort re-resolution vs predictability
`T3` (≥95% heuristic re-resolution, GOAL) competes with the user's ability to reason about where a link
points (`U3` honest behavior). A heuristic can silently retarget a link to the *wrong* resource.
*Physical contradiction — Autonomy vs Control — TRIZ P10/P15/P35.*
> **Resolution:** (Transform) **Confidence-gated** re-resolution. Exact content-hash or OS-bookmark match
> auto-resolves silently; a fuzzy name/path match shows "now points to X (moved from Y) — confirm?" before
> retargeting (P15 dynamization). A rejected match leaves the link broken-but-recorded (T4), never silently
> wrong. Spawns SC-2 (re-resolution confirm UX).
> **Propagation:** T3 TIGHTENED (needs a confidence score gating silent resolve); U3 LOOSENED (reinforces honesty).

### TN6: Sandbox depends on runtime (hidden dependency)
`S1` (deny-by-default sandbox) **depends on** `T1` (Tauri actually enforcing per-plugin JS capability
isolation). T1 is the blocker; S1 cannot be claimed until isolation is proven (seeds DRT-3).
> **Resolution:** Verify-blocker-first. Prove the Tauri capability/IPC mechanism enforces per-plugin
> isolation (spike/test) before asserting S1; if it can't, select an alternative sandbox runtime. Exported
> to m3 as a blocking dependency.

### TN7: Link capture depends on OS permission (hidden dependency)
`B1` (obtain link to active resource) **depends on** `S4` (OS automation/accessibility permission granted).
S4 is the blocker for B1 (seeds DRT-6).
> **Resolution:** The B1 capture path checks S4 permission state and routes to U3 degradation when absent;
> permission state must be runtime-detectable per platform. Exported to m3 as a blocking dependency.

---

## Required Truths

Backward-reasoned from the outcome. Each RT is a TRUTH that must hold (not a build step), with a
`parent` and `[relevance, confidence]` edge. Greenfield, so nothing is SATISFIED yet:
6 NOT_SATISFIED, 4 SPECIFICATION_READY. **Binding constraint: RT-5.**

*(Text above is the original m3-anchor snapshot, kept historical. RT-11 and RT-12 were added
2026-10-04 for GPUI Phase 0 (D22/D23, Task 18): U4 and U5 had no required truth to carry evidence
against until the GPUI shell existed. See each RT below and the current satisfied count via
`manifold status linking-manifesto-app`.)*

### RT-1: A stable, resolvable link to the active resource is obtainable per supported app
For each supported app/platform, the system can produce a link (address + name) to the currently
open/selected resource — the manifesto's literal demand. ← OUTCOME [rel 1.0, conf 1.0]
> **Gap:** No adapter layer exists. The macOS capture path (AppleScript/Accessibility/URL schemes) is unbuilt.
> **conf 1.0:** this IS the manifesto's ask — nothing substitutes for it.

### RT-2: OS automation/accessibility permission is grantable and runtime-detectable
On each platform, the app can request, obtain, and detect at runtime the OS permission needed to
read another app's active resource; absence routes to U3 degradation. ← RT-1 [rel 0.85, conf 0.85]
> **Gap:** No permission flow or runtime detection. (m2 blocking dependency TN7: S4 blocks B1.)
> **conf 0.85:** strong, but files/URLs are linkable without it — partial substitute for a subset of resources.

### RT-3: Resource identity is representable by multiple independently-resolvable identifiers
Each link stores path + OS security-scoped bookmark + content hash + name, enabling re-resolution
when the primary identifier breaks. ← RT-1 [rel 0.85, conf 0.7]
> **Gap:** No resource-identity model. Re-resolution + confidence scoring (per TN5) unbuilt.
> **conf 0.7:** a single canonical identifier is a (brittle) substitute — alternatives exist.

### RT-4: A persistent local store guarantees bidirectional link integrity
Hooking A↔B is atomic and durable; backlinks are queryable from BOTH endpoints; consistent under
concurrent writes. ← OUTCOME [rel 1.0, conf 0.85]
> **Gap:** No graph store. Schema, transactional writes, and concurrency-safety (GAP-17) unbuilt.
> **conf 0.85:** SQLite is one option; some persistent transactional store is necessary, impl substitutable.

### RT-5: A sandbox enforces deny-by-default capabilities on third-party code  ★ BINDING
Third-party plugin code has no ambient network/filesystem/graph access; all access is broker-mediated
to the scope the user granted. ← OUTCOME [rel 1.0, conf 0.7]
> **Gap:** Unproven. Per-plugin isolation depends on the runtime (m2 blocking dependency TN6: T1 blocks S1).
> Resolved by **solution Option B** (separate-process sandbox), but the isolation spike (E9) is not yet run.
> **conf 0.7:** the sandbox *mechanism* has real alternatives (WASM / separate-process / runtime-native) —
> high stakes, multiple paths. This is the hardest RT to close and gates RT-6 and RT-10.

### RT-6: A manifest schema lets plugins declare capabilities, granted per-integration
Plugins declare needed capabilities in a manifest; the user grants them once per integration; the
broker enforces the grant. ← RT-5 [rel 0.85, conf 0.7]
> **Gap:** Manifest schema + grant/consent UX + broker enforcement unbuilt. (Design ready from TN1.)
> **conf 0.7:** the manifest is one design among several declaration formats.

### RT-7: A capability matrix drives honest degradation and scopes B1
A per-app/per-platform descriptor states what linking works where; it scopes B1 (the "supported set")
and powers U3's explained degradation. ← OUTCOME [rel 0.7, conf 0.7]
> **Gap:** No capability descriptor. (Concept defined in TN2/O2.)
> **conf 0.7:** the outcome can technically ship without a *formal* matrix, but the TN2 resolution needs it.
> **Note:** the capability-matrix data path (O2/U3) is SATISFIED and shell-agnostic — unaffected
> by the Tauri→GPUI shell swap (D22). The GPUI panel's own live explained-failure render is a
> separate, additional proof point (`desktop-gpui/probes/capture-view.json`, R1/U3), still PENDING
> pre-Phase 1 — it does not change this RT's SATISFIED status.

### RT-8: Graph and metadata stay on-device with zero background egress
No background phone-home; network access happens only on explicit, user-initiated action.
← OUTCOME [rel 0.7, conf 0.85]
> **Gap:** Architectural choice clear; egress-prevention not yet enforced/tested.
> **conf 0.85:** no substitute for "stays local" once privacy is a committed property.

### RT-9: Panel is instant and link copy takes ≤2 actions (flow preserved)
Hotkey-to-panel-visible meets p99<200ms/p50<80ms and copying a resource's link takes ≤2 user actions.
← OUTCOME [rel 0.7, conf 0.7]
> **Gap:** No UI. Prefetch watcher (TN3) + panel unbuilt.
> **conf 0.7:** performance target has graceful-degradation alternatives; the outcome's behaviors still hold if slower.
> **Status update (GPUI Phase 0, D22):** NOT_SATISFIED → PARTIAL. The GPUI shell itself
> (`desktop-gpui/probes/panel-hotkey.json`) measured hotkey-to-first-frame p50 ≈ 19–21ms /
> p99 ≈ 25–66ms on stub views, debug automation build, 50 triggers — VERIFIED and inside
> U1's budget, but only for the shell, not Phase 1's real views (that full-view, release-build
> measurement is Phase 1's R15 budget probe, PENDING). U2's ≤2-action copy flow is still
> entirely PENDING (`desktop-gpui/probes/copy-link.json`, R2/U2, not yet landed). This
> evidence targets the GPUI shell (D22); the earlier Tauri-panel numbers recorded in
> `.verify.json` were for the now-superseded webview (D8/D11).

### RT-10: Integrations are signed, versioned, and updatable independent of core
Integration bundles carry a verifiable signature, declare a version, and can be installed/updated/
removed without a core release. ← OUTCOME [rel 0.7, conf 0.7]
> **Gap:** No installer, signing/verification, or registry client. (Lifecycle designed in TN4/O1.)
> **conf 0.7:** the ecosystem could bootstrap via other distribution paths.

### RT-11: The GPUI panel's accessibility surface carries the pattern keyboard and screen-reader use need
List/Input/Switch/Dialog each emit an AccessKit node carrying their visible text and role, and
core flows are keyboard-operable, once the accessibility tree is activated. ← OUTCOME [rel 0.7, conf 0.7]
> **Gap:** Proven only on Phase 0's stub spike view (`desktop-gpui/src/views/a11y_spike.rs`); Phase 1's
> real views (capture/browser/settings) must follow the same pattern, and VoiceOver itself has not
> been run.
> **conf 0.7:** the AccessKit pattern has one confirmed gpui-pre 0.3.7 gap (Dialog's own container
> carries no role, pinned as an asserted-absent check per D22) — a future upstream fix is detectable,
> not silently relied on.
> **Added for GPUI Phase 0 (D22/D23):** U4 had no required truth before this shell existed — this RT
> fills that gap. `desktop-gpui/probes/a11y-spike.json` (R7/TN5) is VERIFIED for the AccessKit-role +
> keyboard-navigation pattern on stub views; live VoiceOver parity is explicitly deferred to an
> attended Phase 2 check (PENDING, D22) — the in-process AccessKit tree the probes read is a proof
> surface for agents, not a screen-reader audit.

### RT-12: Backlinks are rendered at the resource in the GPUI panel
Opening or focusing a linked resource surfaces its backlinks newest-first in the panel itself (not
just in the underlying graph store). ← OUTCOME [rel 0.7, conf 0.7]
> **Gap:** No GPUI view renders backlinks yet; Phase 0 shipped only stub views. The bidirectional
> backlink DATA layer is proven separately (RT-4) — this RT is specifically about the panel's render.
> **conf 0.7:** the render is a thin, mostly-mechanical layer over data already proven by RT-4;
> alternatives (e.g. a separate "linked resources" view) could also satisfy U5's intent.
> **Added for GPUI Phase 0 (D22/D23):** U5 had no required truth before this shell existed — this RT
> fills that gap. `desktop-gpui/probes/backlinks.json` (R3/U5) is the Phase 1 acceptance scenario
> (PENDING, not yet landed — fails by design pre-Phase 1).

---

## Solution Space

The options differ in how directly they close the **binding constraint RT-5** (sandbox isolation).
All other architecture (Tauri core, bidirectional graph, broker, capability matrix) is fixed from m1/m2.

### Option A: WASM host-function sandbox
- Satisfies RT-5 most structurally (a plugin can only call imported host functions = the broker by construction); maps cleanly to RT-6.
- Complexity: **High** (WASM toolchain; ship QuickJS-in-WASM to keep JS authoring).
- Reversibility: **REVERSIBLE_WITH_COST** (WASM ABI commitment).

### Option B: Separate-process JS sandbox  ← Selected
- Satisfies RT-5 via OS process isolation; broker over IPC; plugin authors write plain JS (no WASM toolchain).
- Closes the TN6 blocker by deriving isolation from the OS, not a shared JS context.
- Complexity: **Medium-High** (process-per-plugin overhead, IPC plumbing).
- Reversibility: **TWO_WAY** (broker-over-IPC boundary is implementation-agnostic — most swappable later).

### Option C: Tauri-native webview sandbox
- Lowest friction, fits the stack, but webview JS contexts share a runtime → **weakest** per-plugin isolation (the exact TN6 risk). Partially skips the binding constraint.
- Complexity: **Medium**.
- Reversibility: **REVERSIBLE_WITH_COST**.

**Recommendation carried forward: Option B** — strong isolation with the lowest plugin-authoring
friction and the most reversible boundary. (My initial lean was Option A for structural purity; B is
the better cost/reversibility trade for v1 and equally closes RT-5.)

**Tension re-validation:** all 7 resolved tensions remain CONFIRMED under Option B (none reopened).
