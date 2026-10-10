# Spec: GPUI Phase 2a — close the Tauri parity gaps and record the performance budget (unattended)

## Problem
Phase 1 ported the four views to the native GPUI app (`desktop-gpui/`,
`ubilink-next`) and proved each one on the running app (PR #38). Four Tauri
behaviours did not come across. A capture that fails because Automation is not
granted leaves the user on an error instead of onboarding. The panel stays open
after a copy. Left at the browser's top level does nothing. A backlink in the
capture view cannot be opened. Phase 1's T15 also never recorded
`docs/PERFORMANCE.md`, because background load on the host made the hotkey p99
swing between 33 and 154 ms against a frozen doc-match tolerance. Until these
close, the GPUI app cannot replace the Tauri one: the cutover (identity,
packaging, version source, deleting `desktop/` and `dist/`, Makefile and CI) is
Phase 2b, attended, and starts from this phase's result.

## Users
- A keyboard-first macOS user who expects the GPUI panel to behave as the Tauri
  panel did: a permission failure leads to the fix, a copy ends the interaction,
  Left backs out, a backlink is one keystroke from opening.
- The maintainer, who needs these proven on the running app before the attended
  cutover deletes the Tauri app.
- The unattended verifier agent, which drives the app only through the
  automation channel, the probe harness and the verify skill.

## Goals
- Each of the four parity behaviours has a frozen probe written before the run
  (`capture-permission-route`, `copy-dismiss`, `browser-left-close`,
  `backlink-open`) that fails on the baseline and passes on the finished app.
- Every Phase 1 probe still passes on the finished app (R5 re-runs the ones no task re-checks; the integration re-run repeats them).
- `docs/PERFORMANCE.md` records the release app's budget, measured on a quiet
  host, and agrees with `probe.py run performance`.

## Non-goals
- The cutover (Phase 2b, attended): renaming the app or its bundle id, sharing
  the Tauri data dir, packaging and DMG changes, the version source, deleting
  `desktop/`, `dist/` or the node tests, Makefile and CI changes, and the core
  `CaptureFailure::explanation()` how-to-fix text.
- Attended host checks: the real SMAppService round trip, native save and open
  dialogs, VoiceOver, live capture, a real OS hotkey press, a second display.
- Escape at the browser's top level (a separate gap, noted for Phase 2b).
- Adding, removing or bumping any crate or crate feature.
- Changing `ubilink-core`, `ubilink-app`, the graph format or any frozen probe.

## Constraints
- B1 [goal]: Feature parity — the four Tauri behaviours named in Problem work in the GPUI panel.
- U2 [goal]: Copying the captured link takes at most 2 user actions from the hotkey, and the copy ends the interaction.
- U3 [invariant]: Every failure the user can hit shows a non-empty message that says what failed and what to do, or takes the user to where it is fixed.
- U4 [goal]: Every interactive control, backlink rows included, is reachable and operable from the keyboard alone and appears in the accessibility tree with its visible text.
- T1 [invariant]: `src-tauri/` (ubilink-core), `app/` (ubilink-app), `desktop/` and `dist/` are not modified.
- T2 [invariant]: No crate or crate feature is added, removed or bumped; every Cargo.toml and Cargo.lock is unchanged and builds run `--locked`.
- T3 [invariant]: `make verify` passes on the final merged commit.
- T4 [boundary]: `desktop-gpui/probes/`, `desktop-gpui/src/automation/`, `desktop-gpui/.claude/skills/verify-ubilink/scripts/`, `scripts/probe.py`, `scripts/probe-helpers/`, `scripts/ocr.swift`, `Makefile` and `.github/` stay byte-identical to the baseline commit 7fcc3ad548495bfde56ece6696edc8065d1d060f.
- S2 [invariant]: Opening a link goes through the core's single validation policy and the shell's opener seam; automation builds route opens to the opener sink.
- O1 [boundary]: The release app idles at < 0.5 % CPU over 60 s and < 80 MB physical footprint, the stripped release binary is < 20 MB, and hotkey-to-first-frame is p99 < 200 ms and p50 < 80 ms.
- O2 [goal]: A task edits only the files its `[where:]` names, plus their tests.
- O3 [boundary]: The performance budget is measured on a quiet host: the run holds the display awake and the host runs no heavy foreground app during that task.

## Tensions
- TN1 [trade_off]: Dismissing after a copy against the user seeing that the copy happened. (between: U2, U3; status: resolved; strategy: Partition)
  The panel shows "Copied" and stays visible at least 1 s (the frozen view contract), then hides itself within 3 s of the key; the frozen copy-link probe (Copied must be visible after the key) still holds.
- TN2 [trade_off]: Enter already copies the captured link, and Enter is also the natural key to open a backlink. (between: U2, U4; status: resolved; strategy: Partition)
  Focus decides (user decision D4): Tab steps keyboard focus onto backlink rows one at a time; Enter on a focused backlink row opens it; Enter with no backlink row focused still copies.
- TN3 [hidden_dependency]: Routing a failed capture to onboarding against the frozen capture-view probe, which needs a failed capture to stay on capture. (between: B1, U3; status: resolved; strategy: Partition)
  Route only when the permission report is available and says Automation is not granted; any other failure, or no report, stays on capture with its message.
- TN4 [resource_tension]: A frozen doc-match tolerance against hotkey latency that swings with host load. (between: O1, O3; status: resolved; strategy: Prioritize)
  The tolerance stays frozen (user decision D3); the performance task runs last, on a quiet host, with the display held awake, and measures and records in one sitting.
- TN5 [hidden_dependency]: Three tasks edit `views/capture.rs`, whose evidence gate now runs the frozen capture-permission-route probe. (between: O2, B1; status: resolved; strategy: Partition)
  R1 lands first; R2 and R4 come after it, and R3 and R4 come after R2 because R2 and R3 share `shell/mod.rs`.

## Required truths
- RT1 [SPECIFICATION_READY]: The GPUI panel matches the Tauri panel on the four gaps. (parent: OUTCOME; maps_to: B1; reqs: R1, R2, R3, R4; confidence: 0.75; check: {python} scripts/probe.py run capture-permission-route copy-dismiss browser-left-close backlink-open)
- RT2 [SPECIFICATION_READY]: A permission failure leads to its fix. (parent: RT1; maps_to: U3; reqs: R1; confidence: 0.8; check: {python} scripts/probe.py run capture-permission-route capture-view)
- RT3 [SPECIFICATION_READY]: A copy ends the interaction within two actions. (parent: RT1; maps_to: U2; reqs: R2; confidence: 0.75; check: {python} scripts/probe.py run copy-dismiss copy-link)
- RT4 [SPECIFICATION_READY]: Backlinks are keyboard-reachable and open through the core opener. (parent: RT1; maps_to: U4, S2; reqs: R4; confidence: 0.65; check: {python} scripts/probe.py run backlink-open backlinks)
- RT5 [SPECIFICATION_READY]: The core, the Tauri app, the dependency set and the proofs are untouched. (parent: OUTCOME; maps_to: T1, T2, T4; reqs: R1, R2, R3, R4, R5; confidence: 0.9; check: git diff --exit-code 7fcc3ad548495bfde56ece6696edc8065d1d060f -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/.claude/skills/verify-ubilink/scripts desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift Makefile .github)
- RT6 [SPECIFICATION_READY]: The release app meets its budget and the doc records it. (parent: OUTCOME; maps_to: O1, O3; reqs: R5; confidence: 0.6; check: {python} scripts/probe.py run performance)
- RT7 [SPECIFICATION_READY]: The whole gate still passes. (parent: OUTCOME; maps_to: T3, O2; reqs: R5; confidence: 0.85; check: make verify)

## Requirements
- R1: When a capture fails and the app's permission report says Automation is not granted, the panel must switch to the onboarding route in the same window; when Automation is granted or no report is available, a failed capture must stay on the capture route with its failure message. [where: desktop-gpui/src/views/capture.rs, desktop-gpui/src/views/onboarding.rs] [feature: capture-view, onboarding] [proof: with the capture-failed and permissions-two-missing fixtures, `trigger_hotkey capture` makes state report route onboarding with the onboarding steps in the a11y tree, the same window id and an empty opened sink; with permissions-all-granted, and with only Accessibility missing, the panel stays on route capture with view.phase failed and the failure message] [parallel-safe]
- R2: After Enter or ⌘C copies the captured URI, the panel must show the "Copied" confirmation, keep it visible at least 1 s (the frozen view contract in probes/README.md), then hide itself within 3 s of the key. [where: desktop-gpui/src/views/capture.rs, desktop-gpui/src/shell/mod.rs] [feature: copy-link, shell] [proof: for Enter and, on a fresh instance, ⌘C: the pasteboard holds the fixture URI with a higher change count, an a11y "Copied" Label appears, state shows the panel still visible when the probe samples it about 0.8 s after the key (the reviewer holds the code to the 1 s contract), and within 3 s shell.visible is false, route is null and the window has left the screen] [after: R1]
- R3: In the link browser at the top level with an empty filter, Left must close the panel; with a non-empty filter Left must not close it, and while drilled in Left must still go back. [where: desktop-gpui/src/views/browser.rs, desktop-gpui/src/shell/mod.rs] [feature: browser-navigation, shell] [proof: at the top level with an empty filter `key left` makes shell.visible false and the window leave the screen with nothing opened; with filter "be", three Lefts then "x" leave the panel visible with filter "xbe"; drilled in, Left returns to route browser with the panel visible] [after: R2]
- R4: In the capture view, Tab must move keyboard focus onto the backlink rows one at a time, and Enter with a backlink row focused must open that row's URI through the core opener without copying; Enter with no backlink row focused must still copy. [where: desktop-gpui/src/views/capture.rs] [feature: backlinks, copy-link] [proof: with the backlinks graph seeded, Tab until the "Beta Post" row's ListItem carries focused, then `key enter` makes the opened sink hold exactly Beta's URI while the pasteboard still holds the nonce; on a fresh instance with no row focused, Enter puts the captured URI on the pasteboard and opens nothing] [after: R2]
- R5: `docs/PERFORMANCE.md` must record, as measured by `make budget` on a quiet host with the Phase 2a views in place, idle CPU < 0.5 % over 60 s, physical footprint < 80 MB, stripped binary < 20 MB, and hotkey p99 < 200 ms and p50 < 80 ms. [where: docs/PERFORMANCE.md] [feature: resource-budget, panel-hotkey, panel-dismiss] [proof: `probe.py run performance` launches the release app with an isolated data dir, samples it for 60 s, reruns the 50-trigger latency and dismiss probes on the automation build, and checks the numbers are within budget and match PERFORMANCE.md] [after: R3, R4]

## Acceptance criteria
- R1: The capture-permission-route probe exits 0. [cmd: {python} scripts/probe.py run capture-permission-route]
- R1: The capture-view probe still exits 0. [cmd: {python} scripts/probe.py run capture-view]
- R1: The onboarding probe still exits 0. [cmd: {python} scripts/probe.py run onboarding]
- R1: The baseline guard passes. [cmd: git diff --exit-code 7fcc3ad548495bfde56ece6696edc8065d1d060f -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/.claude/skills/verify-ubilink/scripts desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift Makefile .github]
- R2: The copy-dismiss probe exits 0. [cmd: {python} scripts/probe.py run copy-dismiss]
- R2: The copy-link probe still exits 0. [cmd: {python} scripts/probe.py run copy-link]
- R2: The shell probe still exits 0. [cmd: {python} scripts/probe.py run shell]
- R2: The baseline guard passes. [cmd: git diff --exit-code 7fcc3ad548495bfde56ece6696edc8065d1d060f -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/.claude/skills/verify-ubilink/scripts desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift Makefile .github]
- R3: The browser-left-close probe exits 0. [cmd: {python} scripts/probe.py run browser-left-close]
- R3: The browser-drill probe still exits 0. [cmd: {python} scripts/probe.py run browser-drill]
- R3: The baseline guard passes. [cmd: git diff --exit-code 7fcc3ad548495bfde56ece6696edc8065d1d060f -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/.claude/skills/verify-ubilink/scripts desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift Makefile .github]
- R4: The backlink-open probe exits 0. [cmd: {python} scripts/probe.py run backlink-open]
- R4: The backlinks probe still exits 0. [cmd: {python} scripts/probe.py run backlinks]
- R4: The baseline guard passes. [cmd: git diff --exit-code 7fcc3ad548495bfde56ece6696edc8065d1d060f -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/.claude/skills/verify-ubilink/scripts desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift Makefile .github]
- R5: The performance probe exits 0 and agrees with docs/PERFORMANCE.md. [cmd: {python} scripts/probe.py run performance]
- R5: Every Phase 1 scenario no task re-checks still passes with the Phase 2a views in place. [cmd: {python} scripts/probe.py run theming settings-adapters graph-roundtrip graph-reset launch-at-login link-marking link-browser-filter browser-select panel-dismiss panel-hotkey shell]
- R5: The release build carries no automation channel. [cmd: {python} scripts/probe.py release-clean]
- R5: The full gate passes. [cmd: make verify]

## Verification
Verify skill: `desktop-gpui/.claude/skills/verify-ubilink`

## Solution options
- OPT-A: Implement each gap in its view file, adding the smallest public hook to `shell/mod.rs` that hiding the panel needs. (complexity: Low; reversibility: TWO_WAY; satisfies: RT1, RT2, RT3, RT4, RT5, RT6, RT7)
- OPT-B: Move route and dismiss decisions into a new shell-level controller that the views call. (complexity: Medium; reversibility: REVERSIBLE_WITH_COST; satisfies: RT1, RT2, RT3, RT4, RT5, RT6, RT7)
Recommended: OPT-A — both satisfy every truth; OPT-A has lower complexity and touches fewer files, which keeps the tasks inside their [where:] paths.

## Decisions
- D1: How is Phase 2 split? -> Unattended run: the four parity gaps and PERFORMANCE.md. Attended Phase 2b right after: identity (Ubilink, org.dhanesh.ubilink, shared data dir), packaging, version source, core how_to_fix text, deleting desktop/ dist/ and the node tests, Makefile and CI, docs, and the hands-on host checks. (source: user, 2026-10-10; narrowed from "unattended code + attended checks" because Makefile/.github edits prompt and rm -rf is denied in this repo, the unattended lint refuses build-only proofs, and taking org.dhanesh.ubilink while desktop/ exists collides with the Tauri app)
- D2: Which parity gaps? -> All four: missing-Automation to onboarding, dismiss after copy, Left at the top level closes, open a backlink. (source: user, 2026-10-10)
- D3: How is Phase 1's parked T15 resolved? -> Regenerate PERFORMANCE.md on a quiet host; the frozen perf_doc tolerance stays as it is. (source: user, 2026-10-10)
- D4: How is a backlink opened from the keyboard? -> Tab to a backlink row, Enter opens it; Enter with no row focused still copies. (source: user, 2026-10-10)
- D5: Grant scope? -> 72 hours, branch factory/*, push_branch and open_pr granted, max 1 task in parallel, 3 repairs per task, dispatch cap derived. (source: user, 2026-10-10)
- D6: Who writes the acceptance probes? -> The planner, before the grant: the four new scenarios were committed in the prep (1710fff, 7fcc3ad), each fails on the baseline at its new-behaviour step, and the baseline guard freezes them. (source: planner, Phase 0 RT7 pattern)
- D7: How do reviewers judge scope? -> A reviewer fails a task whose diff touches files outside its `[where:]` paths and their tests. (source: Phase 1 D10)
- D8: May the run consult Jev (System One)? -> Yes, for low-stakes calls (naming, triaging whether a failure is related or environmental, whether a concern is in scope); state sent is code, commands and metadata only, never user data or secrets. (source: user's global instructions; user, 2026-10-10)
- D9: What does the verifier do when macOS blocks it? -> Report ENV-BLOCKED, never a product failure; the conductor holds the display awake with caffeinate for the whole run, and the host runs on AC power. (source: Phase 1 lock lesson)

## Iterations
- I1: Drafted from the user's 2026-10-10 decisions and the cutover inventory; narrowed the unattended scope to what a running-app probe can prove (D1); planner-authored probes committed and frozen before the grant (D6); Jev scored the four probes and the fixture 0.33-0.67, so a reasoning pass kept the probes, registered the new fixture. Advisor review: R1 also edits onboarding.rs (the fixture-backed permission report lives there), R2 holds the code to the frozen 1 s contract while the probe samples at about 0.8 s, and R5 re-runs every Phase 1 scenario no task re-checks (607 steps, about 40 s warm on the baseline).

## Open questions
