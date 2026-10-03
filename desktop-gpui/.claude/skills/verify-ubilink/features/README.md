# ubilink feature map

One file per user-facing feature of ubilink-next, in sweep order. Each names its source
anchors (paths relative to `desktop-gpui/`), how to reach it, the exact drive, and what
proves it. A diff touching an anchor makes that feature *touched*, and a touched feature
needs fresh evidence on the new head.

Anchoring rule (final review C1). The conductor (`touched_features`) strips `:Symbol`
and matches anchors by FILE, and every touched feature needs a passing record. So a
file one Phase 1 task edits may anchor only features that task can already prove:
each view file anchors only the FIRST feature of its chain (`views/capture.rs` →
capture-view, `views/browser.rs` → link-browser, `views/onboarding.rs` → onboarding,
`views/settings.rs` → settings-adapters). The later features on the same file
(copy-link, backlinks, browser-navigation, link-marking, graph-data, launch-at-login),
theming and resource-budget anchor only files no Phase 1 task edits (their scenario
JSON, fixtures, `theme/`, `login_item.rs`, `automation/store.rs`); each Phase 1 task's
`[feature:]` tag declares its own feature, so it is still on the gate where it lands.

- [shell](shell.md) — the menu-bar app opens its blurred pop-up panel from a hotkey or the tray and switches views in place
- [capture-view](capture-view.md) — the capture view shows what you just copied, render-first, including what to do when it fails
- [copy-link](copy-link.md) — Enter or Cmd-C on a captured link copies it in at most two actions from the hotkey
- [backlinks](backlinks.md) — the capture view lists who already links to what you just captured, newest first
- [link-browser](link-browser.md) — the link browser renders a virtualised, fuzzy-filtered list even at thousands of links
- [browser-navigation](browser-navigation.md) — arrow keys select, Enter opens, Right drills into a link's connections and Left backs out
- [link-marking](link-marking.md) — Cmd-L marks two links and connects them both ways in the graph
- [onboarding](onboarding.md) — each missing permission gets its own step, its own System Settings pane, and a re-check
- [settings-adapters](settings-adapters.md) — every resource adapter toggles from the keyboard and the choice persists
- [graph-data](graph-data.md) — the graph exports and imports with no change, and Reset asks for confirmation first
- [launch-at-login](launch-at-login.md) — a login-item switch shown and driven through a fixture-fed seam, never the real OS service
- [theming](theming.md) — every view paints in the token-derived light and dark colours and switches live
- [resource-budget](resource-budget.md) — the release app idles light, stays under its footprint, and ships under its size budget
- [panel-hotkey](panel-hotkey.md) — hotkey-to-first-frame stays under its latency budget with the real views in place
- [panel-dismiss](panel-dismiss.md) — capture and browser hide on loss of key status; settings and onboarding don't

## Uncovered Manifold constraints (backlog)

`forge coverage` reads `.manifold/linking-manifesto-app.json` (phase GENERATED). Task 15
mapped the Phase 1 feature files above to every constraint that has a runtime face in
this app's driven UI, moving each `- constraints:` line into the feature file that now
proves it:

- `linking-manifesto-app:B1` → feature `capture-view`, feature `backlinks`
- `linking-manifesto-app:T2` → feature `graph-data`, feature `link-marking`
- `linking-manifesto-app:U1` → feature `panel-hotkey`
- `linking-manifesto-app:U2` → feature `copy-link`
- `linking-manifesto-app:U3` → feature `capture-view`
- `linking-manifesto-app:U4` → feature `browser-navigation`
- `linking-manifesto-app:U5` → feature `backlinks`
- `linking-manifesto-app:S4` → feature `onboarding` (resolved from "Undecided" below: the onboarding view's per-step rationale and pane links are exactly S4's "transparent rationale")

None of these are `proven` yet — the features above all carry `proven: no` until
Phase 1's views replace the stubs and a verifier drives the recipe live; `forge
coverage` counts a constraint as covered once some feature names it in `- constraints:`,
independent of whether that feature is itself proven.

Still undecided (no Phase 1 feature's scenario genuinely proves these; left
uncovered):

- `linking-manifesto-app:S2` Local-only data, zero telemetry — needs a network
  observation this harness does not make (no proxy, no packet capture)
- `linking-manifesto-app:T3` Best-effort link re-resolution via multiple identifiers —
  no Phase 1 requirement exercises cross-identifier re-resolution; this is a future
  feature, not one of the 14 ported here
- `linking-manifesto-app:T5` Resource-addressing behind a platform/app adapter
  interface — Task 15's fix round 1 (I6) re-checked this against
  `probes/settings-adapters.json`'s actual steps and found it only toggles
  adapters on/off and asserts `plugins-state.json`; it never exercises a
  `scheme`-based resource-addressing decision (no step opens a link through a
  specific adapter's scheme), so the earlier "resolved" mapping to
  `settings-adapters` was not genuinely proven and is moved back here

No face in this app's driven UI (packaging, plugins, signing, architecture); expected to
stay uncovered here:

- `linking-manifesto-app:B3` Third parties can add integrations without forking core
- `linking-manifesto-app:O1` Integrations install/update independently of core releases
- `linking-manifesto-app:O2` Per-platform / per-app capability matrix is published and surfaced in-app
- `linking-manifesto-app:S1` Plugin sandbox is deny-by-default
- `linking-manifesto-app:S3` Integrations are signed/verified before install
- `linking-manifesto-app:T1` Tauri architecture with capability-gated IPC (superseded by D22's GPUI shell)
- `linking-manifesto-app:T4` Integrations are versioned, health-checked, and updatable independent of core

## Coverage

Re-run after fix round 1 (`forge coverage desktop-gpui/.claude/skills/verify-ubilink`),
verbatim:

```
COVERED: linking-manifesto-app:B1 -> backlinks, capture-view
COVERED: linking-manifesto-app:S4 -> onboarding
COVERED: linking-manifesto-app:T2 -> graph-data, link-marking
COVERED: linking-manifesto-app:U1 -> panel-hotkey
COVERED: linking-manifesto-app:U2 -> copy-link
COVERED: linking-manifesto-app:U3 -> capture-view
COVERED: linking-manifesto-app:U4 -> browser-navigation
COVERED: linking-manifesto-app:U5 -> backlinks
UNCOVERED: linking-manifesto-app:B3 "Third parties can add integrations without forking core"
UNCOVERED: linking-manifesto-app:O1 "Integrations install/update independently of core releases"
UNCOVERED: linking-manifesto-app:O2 "Per-platform / per-app capability matrix is published and surfaced in-app"
UNCOVERED: linking-manifesto-app:S1 "Plugin sandbox is deny-by-default"
UNCOVERED: linking-manifesto-app:S2 "Local-only data, zero telemetry"
UNCOVERED: linking-manifesto-app:S3 "Integrations are signed/verified before install"
UNCOVERED: linking-manifesto-app:T1 "Tauri architecture with capability-gated IPC"
UNCOVERED: linking-manifesto-app:T3 "Best-effort link re-resolution via multiple identifiers"
UNCOVERED: linking-manifesto-app:T4 "Integrations are versioned, health-checked, and updatable independent of core"
UNCOVERED: linking-manifesto-app:T5 "Resource-addressing behind a platform/app adapter interface"
COVERAGE: 8/18 anchored constraints have an observable proof
```

History: Task 14 left `COVERAGE: 0/18`. Task 15 moved 9 constraints (the 7 "has a
runtime face" ones plus S4 and T5) and reached `9/18`. Fix round 1 (I6) re-checked
every `- constraints:` claim against what its scenario actually asserts and found
`T5` was not genuinely proven by `settings-adapters.json` (it never exercises
scheme-based resource addressing); `T5` moved back to "Undecided" above, landing
at `8/18`. The remaining 8 covered constraints were each re-confirmed against their
scenario's actual steps during the same pass.
