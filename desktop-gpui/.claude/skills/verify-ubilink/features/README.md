# ubilink feature map

One file per user-facing feature of ubilink-next, in sweep order. Each names its source
anchors (paths relative to `desktop-gpui/`), how to reach it, the exact drive, and what
proves it. A diff touching an anchor makes that feature *touched*, and a touched feature
needs fresh evidence on the new head.

- [shell](shell.md) — the menu-bar app opens its blurred pop-up panel from a hotkey or the tray and switches views in place

## Uncovered Manifold constraints (backlog)

`forge coverage` reads `.manifold/linking-manifesto-app.json` (phase GENERATED): 0 of its
18 anchored constraints name a feature here yet. `forge seed` proposed one stub per
constraint; the stubs were not kept, because a stub with `FILL:` markers fails lint and
none can be filled honestly before the Phase 1 views exist. Add a `- constraints:` line
to the feature file that proves each one as it is mapped; re-run `forge coverage`.

Has a runtime face in desktop-gpui (map it with the Phase 1 feature that shows it):

- `linking-manifesto-app:B1` Faithfully realizes the manifesto's two core asks (capture + backlinks)
- `linking-manifesto-app:T2` Bidirectional link graph in a local store (graph round-trip, `ubilink-graph.sqlite`)
- `linking-manifesto-app:U1` Panel invocation feels instant (performance: `metrics` hotkey p50/p99)
- `linking-manifesto-app:U2` Copy-link-to-current-resource in ≤ 2 user actions (capture view, copy link)
- `linking-manifesto-app:U3` Graceful, explained degradation, never silent failure (capture failure message)
- `linking-manifesto-app:U4` Keyboard-first and accessible (key-only drives, a11y roles and labels)
- `linking-manifesto-app:U5` Backlinks are visible at the resource (capture view backlinks)

Undecided: decide when mapping Phase 1 features (S4 is likely onboarding; S2 needs a
network observation this harness does not make):

- `linking-manifesto-app:S2` Local-only data, zero telemetry
- `linking-manifesto-app:S4` Least-privilege OS automation with transparent rationale
- `linking-manifesto-app:T3` Best-effort link re-resolution via multiple identifiers
- `linking-manifesto-app:T5` Resource-addressing behind a platform/app adapter interface

No face in this app's driven UI (packaging, plugins, signing, architecture); expected to
stay uncovered here:

- `linking-manifesto-app:B3` Third parties can add integrations without forking core
- `linking-manifesto-app:O1` Integrations install/update independently of core releases
- `linking-manifesto-app:O2` Per-platform / per-app capability matrix is published and surfaced in-app
- `linking-manifesto-app:S1` Plugin sandbox is deny-by-default
- `linking-manifesto-app:S3` Integrations are signed/verified before install
- `linking-manifesto-app:T1` Tauri architecture with capability-gated IPC (superseded by D22's GPUI shell)
- `linking-manifesto-app:T4` Integrations are versioned, health-checked, and updatable independent of core
