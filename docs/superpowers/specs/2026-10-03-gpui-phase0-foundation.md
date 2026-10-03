# Spec: GPUI Phase 0 — native shell, automation channel and verify skill (attended)

## Problem
The Phase 1 port of ubilink's UI to GPUI is meant to run unattended, and an
unattended run merges a task only after an independent verifier drives the
running app. Today nothing makes that possible: the toolchain (Rust 1.92)
cannot build GPUI, there is no GPUI crate, a native window cannot be driven like
the webview, there is no verify skill, and it is unproven whether a tray, global
hotkeys and a GPUI pop-up panel can share one macOS event loop. Each of those
would stall an unattended run or force it to ask a human, so they are done here,
attended.

## Users
- The maintainer, who approves CI and toolchain changes and grants macOS
  permissions once.
- The Phase 1 executor, reviewer and verifier agents, which need a shell, a
  router, a theme, fixtures and a way to drive and observe the app.

## Goals
- `ubilink-next` (GPUI) builds beside the Tauri app, shows a tray icon, opens a
  blurred pop-up panel from all three hotkeys, and hides its dock icon.
- An agent can launch two isolated instances, drive them, read their view
  state and record evidence, using a verify skill that has been run live once.
- Every Phase 1 requirement already has a probe scenario that fails now and will
  pass once its view is built.
- `make verify` and CI run on Rust 1.98 and include the GPUI crate.

## Non-goals
- Building any of the four views beyond an empty, routable stub (Phase 1).
- Deleting `desktop/`, `dist/` or the node tests, or changing release packaging
  for the DMG (Phase 2).
- Changing `ubilink-core` behaviour or the graph schema.

## Constraints
- T1 [invariant]: `ubilink-core` gains no GUI or network dependency; its `no_network_deps` guard still passes.
- T2 [boundary]: The toolchain is Rust 1.98.x in `rust-toolchain.toml` and every workflow, and the GPUI stack is exactly gpui-kit 0.7.0 (gpui-pre =0.3.7), tray-icon 0.26, global-hotkey 0.8, objc2 0.6, objc2-app-kit 0.3, objc2-service-management 0.3 and interprocess 2.4, with Cargo.lock committed.
- T3 [invariant]: The Tauri app and its tests keep passing `make verify` throughout.
- T4 [goal]: Command logic lives in a GUI-free `ubilink-app` crate that both shells call, so Phase 1 ports UI, not logic.
- S1 [invariant]: The automation channel is compiled only with the `automation` cargo feature, listens on a 0600 Unix socket in a 0700 per-user directory, rejects peers with another uid, and accepts only allowlisted verbs.
- S2 [invariant]: The automation socket and the single-instance socket are separate.
- U1 [boundary]: The shell meets hotkey-to-first-frame p99 < 200 ms on an empty panel, so Phase 1 starts with headroom.
- O1 [goal]: Two instances run side by side, each with its own data directory, socket and fixtures.
- O2 [boundary]: Phase 1 probes and the feature map exist before Phase 1 starts, and a guard detects any edit to them.
- O3 [goal]: The verify skill passes `forge lint` and has been run live on at least one feature at a recorded commit.
- O4 [invariant]: Probes never cause real side effects on the host: opens go to a sink, file dialogs take fixture paths, GUI probes hold a desktop lock, and automation instances do not register real global hotkeys.
- O5 [boundary]: Phase 1 needs no edit to `desktop-gpui/Cargo.toml`: every crate feature the four views need is enabled here.

## Tensions
- TN1 [trade_off]: Drivable runtime proof against a release build with no automation surface. (between: S1, O3; status: resolved; strategy: Partition)
  The `automation` feature builds the probe binary; `probe.py release-clean` proves the release binary has no socket path or verb names.
- TN2 [hidden_dependency]: The toolchain bump edits CI, and CI edits always need a human. (between: T2, T3; status: resolved; strategy: Prioritize)
  The bump happens here, attended, so Phase 1 never touches `.github/`.
- TN3 [trade_off]: Extracting `ubilink-app` touches the working Tauri shell. (between: T3, T4; status: resolved; strategy: Transform)
  A pure move behind the same function signatures; the Tauri commands become one-line wrappers, and `make test-desktop` proves no behaviour changed.
- TN5 [trade_off]: On-screen proof against TCC prompts that would stall an unattended verifier. (between: O3, S1; status: resolved; strategy: Transform)
  Text proofs read GPUI's AccessKit tree in-process (no TCC); only pixel and OCR steps need Screen Recording, checked once by `probe.py doctor` before a run.
- TN4 [hidden_dependency]: tray-icon and global-hotkey need the main thread and a running NSApp loop that GPUI owns. (between: T2, U1; status: resolved; strategy: Transform)
  Both are created inside `Application::run` on the main thread and polled from a GPUI task; the spike requirement proves it, and a failure triggers the fallback in D5.

## Required truths
- RT1 [SPECIFICATION_READY]: Phase 1 can run unattended end to end. (parent: OUTCOME; maps_to: O2, O3, O5; reqs: R2, R11, R14, R15, R19; confidence: 0.7; check: {python} scripts/probe.py list --expect 15 --expect-failing 15)
- RT2 [SPECIFICATION_READY]: GPUI builds and the old app still passes on the new toolchain. (parent: RT1; maps_to: T2, T3; reqs: R1, R2, R16; confidence: 0.8; check: make verify)
- RT3 [SPECIFICATION_READY]: The OS integrations coexist in one event loop and meet the latency boundary. (parent: RT1; maps_to: U1, T2; reqs: R4, R5; confidence: 0.6; check: {python} scripts/probe.py run shell panel-hotkey panel-dismiss)
- RT4 [SPECIFICATION_READY]: The app can be driven and observed on screen without shipping a backdoor. (parent: RT1; maps_to: S1, S2; reqs: R6, R7; confidence: 0.75; check: {python} scripts/probe.py release-clean)
- RT5 [SPECIFICATION_READY]: Logic is shared and the core stays clean. (parent: RT1; maps_to: T1, T4; reqs: R3; confidence: 0.85; check: cargo test --locked --manifest-path app/Cargo.toml)
- RT6 [SPECIFICATION_READY]: Instances are isolated, seeded and side-effect free. (parent: RT1; maps_to: O1, O4; reqs: R8, R9, R10; confidence: 0.75; check: {python} scripts/probe.py doctor)
- RT7 [SPECIFICATION_READY]: Views start from the tokens and their own module. (parent: RT1; maps_to: O2; reqs: R12, R13; confidence: 0.85; check: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation theme_tokens_convert)
- RT8 [SPECIFICATION_READY]: The decision record and constraint model match the new shell. (parent: OUTCOME; maps_to: T3; reqs: R17, R18; confidence: 0.9; check: make verify-spec)

## Requirements
- R1: The toolchain must be Rust 1.98.x with edition 2024 available, set in `rust-toolchain.toml` and every workflow's toolchain input, with core, registry and desktop crates passing clippy on it. [where: rust-toolchain.toml, .github/workflows/]
- R2: A new crate `desktop-gpui/` must build `ubilink-next.app` (bundle id `org.ubilink.next`, stable dev signing identity) from the pinned stack with every objc2 and gpui-kit feature the four views need enabled, a stripped LTO release profile, and its Cargo.lock committed. [where: desktop-gpui/, scripts/bundle-next.sh] [after: R1]
- R3: The 21 command bodies in `desktop/src/main.rs` must move into a GUI-free `ubilink-app` crate that both shells call, with no tauri or gpui crate in its dependency tree. [where: app/, desktop/src/main.rs] [after: R1]
- R4: The GPUI shell must show a tray item with a menu, register ⌘⇧K, ⌘⇧L and ⌘⇧, globally, open a `WindowKind::PopUp` panel with a blurred background on the display under the cursor, and run with the accessory activation policy, all in one process. [where: desktop-gpui/src/shell/] [after: R2, R3]
- R5: The shell panel must reach first frame from a hotkey with p99 < 200 ms and p50 < 80 ms over 50 triggers, and must hide on loss of key status in capture and browser while staying open in onboarding and settings. [where: desktop-gpui/src/shell/] [after: R4]
- R6: An automation channel compiled only under the `automation` feature must serve open_view, key, state, metrics, trigger_hotkey, resign_key, set_appearance, load_fixture, window_id, a11y (activating and dumping GPUI's AccessKit tree in-process) and opened (the opener sink) over a uid-checked 0600 Unix socket, separate from the single-instance socket. [where: desktop-gpui/src/automation/] [after: R4]
- R7: A spike must show that gpui-kit's list, input, switch and dialog components emit accessibility nodes with their visible text once the tree is activated in-process. [where: desktop-gpui/src/automation/] [after: R6]
- R8: `scripts/probe.py` must launch an isolated instance (own data dir, socket, fixtures, fixture file paths and no real hotkeys), run scenario files with the steps see, ocr, pixel, pasteboard, graph, file, winframe, warp_cursor and activate_other, restart an instance on request, run two side by side under a desktop lock, and seed each build from a warm target directory with an APFS clone. [where: scripts/probe.py, scripts/ocr.swift] [after: R6]
- R9: `probe.py doctor` must report Screen Recording access, display awake, display count, a compiled OCR helper and an active caffeinate assertion, and exit non-zero when any is missing. [where: scripts/probe.py] [after: R8]
- R10: Fixtures must exist for a 5,000-link graph, a 20-query golden ranking table, a 3-backlink timestamped seed with a decoy, permission states, a delayed capture, a failed capture, and an export path. [where: desktop-gpui/probes/fixtures/] [after: R8]
- R11: Probe scenarios for all 15 Phase 1 checks must exist under `desktop-gpui/probes/`, each asserting view-specific accessibility content and each failing against the stub views. [where: desktop-gpui/probes/] [after: R9, R10]
- R12: A converter must turn `design/ubilink-tokens.json` into gpui-kit light and dark theme configs that the shell loads at start. [where: desktop-gpui/src/theme/] [after: R2]
- R13: A router table must register four stub views (capture, browser, onboarding, settings), each in its own file under `desktop-gpui/src/views/` exposing a serialisable view-state for the `state` verb. [where: desktop-gpui/src/views/] [after: R4]
- R14: A verify skill at `desktop-gpui/.claude/skills/verify-ubilink` must pass `forge lint` and be run live once on the shell feature, with its evidence recorded at the driven commit. [where: desktop-gpui/.claude/skills/verify-ubilink] [after: R11]
- R15: The verify skill's feature map must contain one entry for each of the 14 Phase 1 feature ids plus `shell`. [where: desktop-gpui/.claude/skills/verify-ubilink/features/] [after: R14]
- R16: `make verify` must include a `test-gpui` target and `probe.py release-clean`, and CI must run it on macos-latest. [where: Makefile, .github/workflows/ci.yml] [after: R2, R6]
- R17: DECISIONS.md must record D22 (GPUI shell, superseding D8 and D11, with the gpui-pre zed revision 1a28cff) and D23 (automation channel and its security rules). [where: DECISIONS.md] [after: R6]
- R18: The Manifold model must name the GPUI shell and probe evidence for U1–U5 so `make verify-spec` passes. [where: .manifold/] [after: R17]
- R19: The final Phase 0 commit must be tagged `phase0-done`, and every `phase0-done` in the Phase 1 spec must be replaced by that commit's literal sha before the Phase 1 grant is written. [where: docs/superpowers/specs/2026-10-03-gpui-phase1-port.md] [after: R15, R16, R18]

## Acceptance criteria
- R1: `rustc --version` reports 1.98 inside the repo and no workflow still names 1.92. [cmd: {python} scripts/probe.py toolchain --expect 1.98]
- R1: The existing gate passes on the new toolchain. [cmd: make verify]
- R2: The bundled app builds locked and its Info.plist carries org.ubilink.next. [cmd: bash scripts/bundle-next.sh dev]
- R3: The app crate's tests pass. [cmd: cargo test --locked --manifest-path app/Cargo.toml]
- R3: The dependency guard finds no tauri or gpui crate under ubilink-app. [cmd: {python} scripts/probe.py deps-clean app/Cargo.toml]
- R3: The Tauri shell's tests still pass after the move. [cmd: make test-desktop]
- R4: The shell probe sees tray_present, three hotkeys registered, window kind popup, background blurred and activation accessory. [cmd: {python} scripts/probe.py run shell]
- R4: The maintainer presses each real hotkey with the cursor on a second display and sees the panel open there, before and after one sleep/wake cycle.
- R5: The panel-hotkey and panel-dismiss probes exit 0 against the stub views. [cmd: {python} scripts/probe.py run panel-hotkey panel-dismiss]
- R6: The release build contains no automation socket path or verb names. [cmd: {python} scripts/probe.py release-clean]
- R6: Headless test `automation_rejects_unknown_verb` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation automation_rejects_unknown_verb]
- R7: The a11y spike probe sees nodes with visible text for list, input, switch and dialog. [cmd: {python} scripts/probe.py run a11y-spike]
- R8: Two instances run side by side under the lock and each answers `state`, and an `opened` call reports the sink without launching anything. [cmd: {python} scripts/probe.py run shell --instances 2]
- R9: Doctor passes on the maintainer's Mac. [cmd: {python} scripts/probe.py doctor]
- R10: The fixture check finds every fixture and validates the 5,000-link graph and the 20-query table. [cmd: {python} scripts/probe.py fixtures --check]
- R11: All 15 Phase 1 scenarios are listed and every one fails against the stubs. [cmd: {python} scripts/probe.py list --expect 15 --expect-failing 15]
- R12: Headless test `theme_tokens_convert` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation theme_tokens_convert]
- R13: Headless test `router_registers_four_views` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation router_registers_four_views]
- R14: `forge lint` prints LINT_RESULT: PASS for the verify skill, and `.verify/` holds an evidence.json for the shell feature naming the driven commit.
- R15: `forge lint` passes and `features/README.md` lists 15 entries matching the 14 Phase 1 feature ids plus shell.
- R16: The gate runs the GPUI tests and the release-clean check. [cmd: make verify]
- R16: CI is green on the pull request on macos-latest.
- R17: DECISIONS.md has D22 and D23 headings. [cmd: grep -c "^## D2[23] " DECISIONS.md]
- R18: The Manifold spec validates. [cmd: make verify-spec]
- R19: The tag exists and the Phase 1 spec no longer names it. [cmd: git rev-parse --verify phase0-done]
- R19: `grep -c phase0-done docs/superpowers/specs/2026-10-03-gpui-phase1-port.md` prints 0, and the Phase 1 spec still passes `spec_lint.py --unattended`.

## Verification
Verify skill: `desktop-gpui/.claude/skills/verify-ubilink`

## Solution options
- OPT-A: gpui-kit 0.7.0 over its pinned gpui-pre =0.3.7 snapshot from crates.io, with tray-icon, global-hotkey and objc2 shims. (complexity: Medium; reversibility: REVERSIBLE_WITH_COST; satisfies: RT1, RT2, RT3, RT4, RT5, RT6, RT7, RT8)
- OPT-B: A git dependency on zed-industries/zed pinned to a revision, plus gpui-kit built against it. (complexity: High; reversibility: REVERSIBLE_WITH_COST; satisfies: RT1, RT2, RT3, RT4, RT5, RT6, RT7, RT8)
- OPT-C: crates.io `gpui` 0.2.2 with no component kit. (complexity: Medium; reversibility: REVERSIBLE_WITH_COST; satisfies: RT1, RT4, RT5, RT6, RT8)
Recommended: OPT-A — satisfies every truth at Medium complexity; OPT-B must track the exact zed revision gpui-kit was built for, and OPT-C is a year stale and incompatible with gpui-kit.

## Decisions
- D1: Phasing? -> Three phases; this is Phase 0, attended. (source: user, 2026-10-03)
- D2: Dependency policy? -> Exactly the pinned stack in T2, Cargo.lock committed; nothing else. (source: user, 2026-10-03)
- D3: Where do view-state dumps come from? -> The app's own serialisable view-models (Rust ports of the panel-state and browse-state reducers), never GPUI's element tree. (source: Opus advisor review)
- D4: Where do the new artifacts live? -> `desktop-gpui/` beside `desktop/`; binary `ubilink-next`; bundle id `org.ubilink.next`; Tauri stays until Phase 2. (source: Opus advisor review, accepted by planner)
- D5: What if tray-icon or global-hotkey cannot share GPUI's loop? -> Fall back to a small objc2 NSStatusItem and Carbon RegisterEventHotKey shim inside the shell, and record it in D22. (source: planner)
- D6: Which on-screen proof surface? -> GPUI's AccessKit tree read in-process for text, roles and flags; screencapture plus Vision OCR and pixel sampling only for colour and evidence; not VisualTestContext or render_to_image, which run off the real app or pull test-support into the build. (source: Opus advisor review of gpui-pre 0.3.7 source)

## Iterations
- I1: Drafted from the Opus advisor's Phase 0 list; added the probe guard (O2) so Phase 1 cannot weaken its own proofs.
- I2: Split the spike (R4) from the automation channel (R5) and ordered the toolchain first after TN2.
- I3: After the adversarial review: added the a11y and opened verbs, the a11y spike, probe steps and desktop lock, doctor, fixtures, a bundled .app with every needed crate feature, moved hotkey and dismiss proofs here (R5), and added the baseline pin (R19).

## Open questions
