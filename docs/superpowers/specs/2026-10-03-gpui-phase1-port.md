# Spec: GPUI Phase 1 — port the four views and raise the UX bar (unattended)

## Problem
Ubilink's panel is a Tauri webview over a hand-written web UI: a second process
(WebKit), a 26 MB shell, and a UI layer that can only be checked in a browser
preview. Users feel it as a panel that is not quite native (vibrancy faked in
CSS, a list that renders every row, filtering by substring only). Phase 0 built a
native GPUI shell (`desktop-gpui/`, `ubilink-next.app`) with a tray, global
hotkeys, a blurred pop-up panel that dismisses natively, a router with one stub
module per view, a token-derived theme, an automation channel, fixtures and a
probe harness. Phase 1 fills those views so the GPUI app can replace the Tauri
one in Phase 2.

## Users
- A keyboard-first macOS user who presses ⌘⇧K to copy a link to whatever is in
  front, ⌘⇧L to browse and connect links, and ⌘⇧, to manage the app.
- The maintainer, who needs every view proven in the running app before the
  Tauri shell is deleted.
- The unattended verifier agent, which drives the app only through the
  automation channel, the probe harness and the verify skill.

## Goals
- Every Tauri view (capture, link browser, onboarding, settings) has a GPUI
  equivalent whose probe passes against the running `ubilink-next.app`, judged
  on the rendered accessibility tree, not only on the view-model.
- The panel opens in p99 < 200 ms and p50 < 80 ms from hotkey to first frame
  with the real views in place.
- A 5,000-link graph browses and filters within a 16 ms frame.
- The release app idles at < 0.5 % CPU and < 80 MB physical footprint, and its
  stripped binary is < 20 MB.

## Non-goals
- Deleting `desktop/`, `dist/` or the node tests, renaming `ubilink-next`, or
  changing packaging and CI (Phase 2, attended).
- Adding, removing or bumping any crate or crate feature (fixed in Phase 0).
- Changing `ubilink-core` or `ubilink-app` behaviour, schemas or the graph
  format.
- A VoiceOver audit: the accessibility tree is used as a proof surface, but
  screen-reader parity is a Phase 2 attended check (D7).
- The plugin runtime, non-macOS builds, auto-update.

## Constraints
- B1 [goal]: Feature parity — every user action available in the Tauri panel is available in the GPUI panel.
- U1 [boundary]: Hotkey-to-first-frame latency, timed at Metal present, is p99 < 200 ms and p50 < 80 ms over 50 triggers (manifold U1).
- U2 [goal]: Copying the captured link takes at most 2 user actions from the hotkey (manifold U2).
- U3 [invariant]: Every failure the user can hit shows a non-empty message that says what failed and what to do (manifold U3).
- U4 [goal]: Every interactive control is reachable and operable from the keyboard alone and appears in the accessibility tree with its visible text.
- T1 [invariant]: `src-tauri/` (ubilink-core), `app/` (ubilink-app), `desktop/` and `dist/` are not modified, so the core's guards and the Tauri app stay as they were.
- T2 [invariant]: No crate or crate feature is added, removed or bumped; `desktop-gpui/Cargo.toml` and every Cargo.lock are unchanged and builds run `--locked`.
- T3 [invariant]: `make verify` passes on the final merged commit, including the Tauri crate and the `dist/` node tests.
- T4 [boundary]: `desktop-gpui/probes/`, `desktop-gpui/src/automation/`, `scripts/probe.py`, `scripts/probe-helpers/`, `scripts/ocr.swift` and `.github/` stay byte-identical to the Phase 0 baseline commit.
- S1 [invariant]: The automation channel exists only in builds with the `automation` feature; the release binary contains no automation socket path or verb names.
- S2 [invariant]: Opening, importing and capturing links go through the core's single validation policy (D16); the GPUI shell adds no opener of its own, and automation builds route opens to the opener sink.
- O1 [boundary]: The release app idles at < 0.5 % CPU over 60 s and < 80 MB physical footprint, and the stripped release binary is < 20 MB.
- O2 [goal]: Each view lives in its own module file under `desktop-gpui/src/views/`; a task edits only the files its `[where:]` names, plus tests.

## Tensions
- TN1 [trade_off]: Rich views (backlinks, live capture) against first-frame latency. (between: B1, U1; status: resolved; strategy: Transform)
  Render-first, fill-async: the view opens in a `capturing` state with no capture data and fills when the core answers, the same reducer split as D8.
- TN2 [resource_tension]: gpui-kit components add code size against the binary budget. (between: B1, O1; status: resolved; strategy: Prioritize)
  O1 wins: only the gpui-kit modules a view needs are used; R15 measures the release build last and fails the run if it is over.
- TN3 [hidden_dependency]: Runtime proof needs an automation channel, but the release build must not carry one. (between: S1, T4; status: resolved; strategy: Partition)
  Probes drive the `automation` build; R15 measures and scans the release build.
- TN4 [trade_off]: Parallel tasks against shared files and one shared desktop (pasteboard, key window, cursor). (between: O2, T3; status: resolved; strategy: Partition)
  Same-file tasks are ordered with `[after:]`; probe.py holds a desktop lock so GUI probes never overlap; automation instances skip real hotkey registration.
- TN5 [trade_off]: Proving every task with the full gate against a 24-hour grant. (between: T3, O2; status: resolved; strategy: Partition)
  Each task runs its own probes, headless tests and the baseline guard; the full `make verify` runs on R15 and again at the conductor's integration re-run.

## Required truths
- RT1 [SPECIFICATION_READY]: The GPUI app is a drop-in replacement for the Tauri panel. (parent: OUTCOME; maps_to: B1; reqs: R1, R4, R9, R10; confidence: 0.75; check: {python} scripts/probe.py run capture-view link-browser onboarding settings-adapters)
- RT2 [SPECIFICATION_READY]: The capture flow copies a link within two actions and explains every failure. (parent: RT1; maps_to: U2, U3; reqs: R1, R2, R3; confidence: 0.8; check: {python} scripts/probe.py run capture-view copy-link backlinks)
- RT3 [SPECIFICATION_READY]: The panel still meets the latency boundary with the real views. (parent: RT1; maps_to: U1; reqs: R15; confidence: 0.65; check: {python} scripts/probe.py run performance)
- RT4 [SPECIFICATION_READY]: The link browser stays within a frame at 5,000 links and is fully keyboard-driven. (parent: RT1; maps_to: U4, B1; reqs: R4, R5, R6, R7, R8; confidence: 0.7; check: {python} scripts/probe.py run link-browser link-browser-filter browser-select browser-drill link-marking)
- RT5 [SPECIFICATION_READY]: The trusted core, the Tauri app and the dependency set are untouched. (parent: OUTCOME; maps_to: T1, T2, S2; reqs: R15; confidence: 0.9; check: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock)
- RT6 [SPECIFICATION_READY]: The old app keeps working until Phase 2 removes it. (parent: OUTCOME; maps_to: T3; reqs: R15; confidence: 0.85; check: make verify)
- RT7 [SPECIFICATION_READY]: Proofs cannot be weakened by the tasks they judge. (parent: OUTCOME; maps_to: T4; reqs: R1, R15; confidence: 0.85; check: git diff --exit-code phase0-done -- desktop-gpui/probes desktop-gpui/src/automation scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github)
- RT8 [SPECIFICATION_READY]: The automation channel never ships. (parent: OUTCOME; maps_to: S1; reqs: R15; confidence: 0.85; check: {python} scripts/probe.py release-clean)
- RT9 [SPECIFICATION_READY]: The release app is lighter than a webview shell at idle. (parent: OUTCOME; maps_to: O1; reqs: R15; confidence: 0.6; check: {python} scripts/probe.py run performance)
- RT10 [SPECIFICATION_READY]: Views are isolated so parallel tasks merge cleanly. (parent: RT1; maps_to: O2; reqs: R1, R4, R9, R10, R14; confidence: 0.75; check: the reviewer compares `git diff --name-only` of the task against its [where:] paths)
- RT11 [SPECIFICATION_READY]: The app looks native in both appearances and settings changes are durable. (parent: RT1; maps_to: B1, U4; reqs: R11, R12, R13, R14; confidence: 0.7; check: {python} scripts/probe.py run theming graph-roundtrip graph-reset launch-at-login)

## Requirements
- R1: The capture view must open in the `capturing` state before the core answers, then render the frontmost resource's name and URI, and must show a message saying what failed and what to do when capture fails. [where: desktop-gpui/src/views/capture.rs] [feature: capture-view] [proof: with the 1 s delayed capture fixture, `trigger_hotkey capture` shows an a11y "Capturing" node in the first frame, then a11y and OCR show the fixture name and URI; with the failure fixture an a11y error node states the failure and the next step] [parallel-safe]
- R2: Pressing Enter or ⌘C in the captured state must put the URI on the macOS pasteboard and show a copied confirmation, so copying takes at most two actions from the hotkey. [where: desktop-gpui/src/views/capture.rs] [feature: copy-link] [proof: probe puts a nonce on the pasteboard, then hotkey + `key enter` makes `pbpaste` print the fixture URI with a higher change count and an a11y "Copied" node; repeated with `key cmd-c`] [after: R1]
- R3: The capture view must list the captured resource's backlinks from the graph in recency order. [where: desktop-gpui/src/views/capture.rs] [feature: backlinks] [proof: with a graph seeded with 3 backlinks at distinct known timestamps and 1 unrelated decoy, the a11y backlink list holds exactly the 3 URIs newest first and the decoy is absent; after the fixture adds a fourth, newer backlink and the view reopens, it appears first] [after: R2]
- R4: The link browser must render the graph through a virtualised list that builds at most 60 row elements at 5,000 links and shows its first frame in under 100 ms. [where: desktop-gpui/src/views/browser.rs] [feature: link-browser] [proof: with the 5,000-link fixture, a11y holds at most 60 row nodes whose text matches fixture rows, `key end` brings row 5000 into the a11y tree, and metrics.browser_first_frame_ms < 100 timed at present] [parallel-safe]
- R5: The link browser filter must rank in-order subsequence matches above other matches, matching the 20-query golden ranking table in the probe fixtures, and must re-filter 5,000 links with p99 < 16 ms per keystroke. [where: desktop-gpui/src/views/browser.rs] [feature: link-browser] [proof: for each of the 20 golden queries, typing it makes the a11y rows' top 3 equal the table's expected top 3 with every non-subsequence decoy ranked below, and a 50-key typing script reports metrics.filter_p99_ms < 16] [after: R4]
- R6: In the link browser, Up and Down must move the selection and Enter must open the selected link through the core opener. [where: desktop-gpui/src/views/browser.rs] [feature: browser-navigation] [proof: after `key down down`, a11y row 3 carries the selected flag; after `key up`, row 2 does; `key enter` makes the `opened` sink report row 2's URI] [after: R5]
- R7: In the link browser, Right must drill into the selected link's connections with a visible trail, and Left must return with the previous selection restored. [where: desktop-gpui/src/views/browser.rs] [feature: browser-navigation] [proof: `key right` makes a11y show the trail with the parent's name and the child list; `key left` empties the trail and row 2 carries the selected flag again] [after: R6]
- R8: Pressing ⌘L on one link and then on another must create a bidirectional link between them in the graph and show that it did. [where: desktop-gpui/src/views/browser.rs] [feature: link-marking] [proof: after marking A then B, an a11y node confirms the link, the instance graph has links_of(A) ∋ B and links_of(B) ∋ A, and after a restart drilling into B lists A] [after: R7]
- R9: The onboarding view must show each missing permission as a step that opens its System Settings pane, and Re-check must update the steps in the same window. [where: desktop-gpui/src/views/onboarding.rs] [feature: onboarding] [proof: with the two-missing-permissions fixture, a11y shows two steps; activating the Automation step makes the `opened` sink report its exact pane URL; after flipping the fixture and `key r`, a11y shows it granted with the same window id] [parallel-safe]
- R10: The settings view must toggle each app adapter from the keyboard and persist the choice to `plugins-state.json` across a restart. [where: desktop-gpui/src/views/settings.rs] [feature: settings-adapters] [proof: for every adapter, a keyboard toggle sets its a11y switch to off and writes plugins-state.json; after a restart it is still off; toggling back on survives a restart too] [parallel-safe]
- R11: The settings view must export the graph to a JSON file and import it back with no change in the graph. [where: desktop-gpui/src/views/settings.rs] [feature: graph-data] [proof: export to the fixture-supplied path yields JSON whose link set equals the seed; after an empty-graph fixture and import, graph_stats and the a11y counts equal the original] [after: R10]
- R12: The settings view must reset the graph only after an explicit confirmation step. [where: desktop-gpui/src/views/settings.rs] [feature: graph-data] [proof: Reset shows an a11y confirmation dialog; Cancel leaves graph_stats unchanged; Confirm makes graph_stats report 0 links and the a11y count read 0] [after: R11]
- R13: The settings view must offer a launch-at-login toggle backed by `SMAppService.mainApp` through a status seam, and must show each status the seam reports (not-registered, enabled, requires-approval with an explanation, not-found with an explanation). [where: desktop-gpui/src/views/settings.rs] [feature: launch-at-login] [proof: with the login_item fixture driving the automation seam, toggling on records a register call and a11y shows the fixture's resulting status (enabled, then requires-approval with its explanation, then not-found with its explanation), the shown status survives a restart, and toggling off records an unregister call and shows not-registered] [after: R12]
- R14: Every view must draw its surfaces, text and accents in the token-derived light and dark colours and must switch them when the system appearance changes, without a restart. [where: desktop-gpui/src/views/] [feature: theming] [proof: for each of the 4 views and both appearances, after `set_appearance` the pid and window id are unchanged, and pixel samples of an opaque surface, a primary text run and the accent control are each within ΔE 3 of the converter's tokens for that mode, with screenshots of all 8 states recorded] [after: R3, R8, R9, R13]
- R15: A `make budget` target must measure the release app and `docs/PERFORMANCE.md` must record its idle CPU < 0.5 % over 60 s, physical footprint < 80 MB, stripped binary < 20 MB and hotkey p99 < 200 ms and p50 < 80 ms with the real views. [where: Makefile, docs/PERFORMANCE.md] [feature: resource-budget, panel-hotkey, panel-dismiss] [proof: `probe.py run performance` launches the release app (not the automation build) with an isolated data dir, samples it for 60 s, reruns the 50-trigger latency and dismiss probes on the automation build, and checks the numbers are within budget and match PERFORMANCE.md] [after: R14]

## Acceptance criteria
- R1: Headless view-model test `capture_view_states` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation capture_view_states]
- R1: The capture-view probe exits 0. [cmd: {python} scripts/probe.py run capture-view]
- R1: Nothing under the baseline-protected paths changed. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R2: The copy-link probe exits 0. [cmd: {python} scripts/probe.py run copy-link]
- R2: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R3: The backlinks probe exits 0. [cmd: {python} scripts/probe.py run backlinks]
- R3: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R4: Headless test `browser_virtualises_rows` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation browser_virtualises_rows]
- R4: The link-browser probe exits 0. [cmd: {python} scripts/probe.py run link-browser]
- R4: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R5: Headless test `fuzzy_filter_ranks_subsequence` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation fuzzy_filter_ranks_subsequence]
- R5: The link-browser-filter probe exits 0. [cmd: {python} scripts/probe.py run link-browser-filter]
- R5: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R6: The browser-select probe exits 0. [cmd: {python} scripts/probe.py run browser-select]
- R6: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R7: The browser-drill probe exits 0. [cmd: {python} scripts/probe.py run browser-drill]
- R7: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R8: The link-marking probe exits 0. [cmd: {python} scripts/probe.py run link-marking]
- R8: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R9: The onboarding probe exits 0. [cmd: {python} scripts/probe.py run onboarding]
- R9: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R10: The settings-adapters probe exits 0. [cmd: {python} scripts/probe.py run settings-adapters]
- R10: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R11: The graph-roundtrip probe exits 0. [cmd: {python} scripts/probe.py run graph-roundtrip]
- R11: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R12: The graph-reset probe exits 0. [cmd: {python} scripts/probe.py run graph-reset]
- R12: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R13: The launch-at-login probe exits 0. [cmd: {python} scripts/probe.py run launch-at-login]
- R13: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R14: Headless test `theme_matches_tokens` passes. [cmd: cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation theme_matches_tokens]
- R14: The theming probe exits 0. [cmd: {python} scripts/probe.py run theming]
- R14: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R15: The performance probe exits 0 and agrees with docs/PERFORMANCE.md. [cmd: {python} scripts/probe.py run performance]
- R15: The release build carries no automation channel. [cmd: {python} scripts/probe.py release-clean]
- R15: The baseline guard passes. [cmd: git diff --exit-code phase0-done -- src-tauri app desktop dist desktop-gpui/probes desktop-gpui/src/automation desktop-gpui/Cargo.toml desktop-gpui/Cargo.lock scripts/probe.py scripts/probe-helpers scripts/ocr.swift .github]
- R15: The full gate passes. [cmd: make verify]

## Verification
Verify skill: `desktop-gpui/.claude/skills/verify-ubilink`

## Solution options
- OPT-A: Build each view from gpui-kit 0.7 components (virtual_list, input, switch, setting, dialog, notification) over the Phase 0 shell. (complexity: Medium; reversibility: REVERSIBLE_WITH_COST; satisfies: RT1, RT2, RT3, RT4, RT5, RT6, RT7, RT8, RT9, RT10, RT11)
- OPT-B: Hand-roll every view from raw GPUI elements with no component kit. (complexity: High; reversibility: REVERSIBLE_WITH_COST; satisfies: RT1, RT2, RT3, RT4, RT5, RT6, RT7, RT8, RT9, RT10, RT11)
- OPT-C: Keep the Tauri webview and restyle the web UI. (complexity: Low; reversibility: TWO_WAY; satisfies: RT5, RT6, RT7, RT8)
Recommended: OPT-A — the only Medium option that satisfies every required truth; OPT-C cannot meet RT1 or RT9 (no GPUI, a second WebKit process), and OPT-B does the same job at higher cost.

## Decisions
- D1: How should the port be phased? -> Three phases: Phase 0 attended foundation, Phase 1 unattended view port (this spec), Phase 2 attended cutover. (source: user, 2026-10-03; Jev phasing score 2.32/3 at confidence 0.32; Opus advisor review)
- D2: Which UX improvements beyond parity? -> All four: resource budget gate, fuzzy search at 5k+ rows, native token theming, launch at login. (source: user, 2026-10-03)
- D3: May the run add or bump dependencies? -> No. The Phase 0 stack and its crate features are fixed (gpui-kit 0.7.0 with gpui-pre =0.3.7, tray-icon 0.26, global-hotkey 0.8, objc2 0.6, objc2-app-kit 0.3, objc2-service-management 0.3, interprocess 2.4); the baseline guard fails any change to desktop-gpui/Cargo.toml or a Cargo.lock. (source: user, 2026-10-03)
- D4: Grant scope? -> 24 hours, branch factory/*, push_branch and open_pr granted, max 2 tasks in parallel, 2 repairs per task, dispatch cap derived. (source: user, 2026-10-03)
- D5: What does the verifier do when macOS blocks it (TCC prompt, locked session, no Screen Recording grant, one display)? -> Report ENV-BLOCKED, never a product failure; text proofs use the in-process accessibility tree so only pixel and multi-display checks can be ENV-BLOCKED; the run starts only after `probe.py doctor` passes. (source: Opus advisor reviews)
- D6: How does the shell treat SMAppService's statuses, and how is R13 proven without touching the host? -> Requires-approval and not-found are valid states shown with an explanation, not failures. In automation builds SMAppService calls go through a seam fed by the `login_item` fixture, so probes never register a real login item on the host (O4); the real SMAppService round trip is an attended Phase 2 check. (source: planner ruling R-o, after Task 11 found an unbundled probe binary cannot register and real registration is a host side effect)
- D7: Is VoiceOver parity in Phase 1? -> No; keyboard reach and accessible text (U4) are in, a VoiceOver pass is an attended Phase 2 check. (source: planner, from the research's UNVERIFIED AccessKit finding)
- D8: May the run consult Jev (System One)? -> Yes, for low-stakes calls (naming, choosing which fixture or test case to add, triaging whether a failure is related to the task); state sent is code, commands and metadata only, never user data or secrets. (source: user's global instructions)
- D9: What does `phase0-done` mean in the guard commands? -> Before the grant is written, every `phase0-done` in this spec is replaced by the literal Phase 0 baseline commit sha, so the grant's spec hash pins the baseline and no executor can move it. (source: planner, after the Opus advisor's guard finding)
- D10: How do reviewers judge scope? -> A reviewer fails a task whose diff touches files outside its `[where:]` paths and their tests. (source: Opus advisor review)

## Iterations
- I1: Drafted from the Opus advisor's 14 requirements plus the user's four UX picks; split filter from virtualisation, added dismiss and the untouched guard.
- I2: Added TN3/TN4 after the pre-mortem (proof needs automation; parallel tasks collide); pinned probes as Phase 0 artifacts (T4).
- I3: Serialised same-file tasks after the wave plan showed parallel edits to one file.
- I4: Jev scored every proof < 0.7; rebuilt proofs on the in-process accessibility tree plus OCR and pixels, split navigation, export/reset and budget, moved hotkey and dismiss proofs to Phase 0 and into R15's regression run, pinned the guard to a baseline commit, and kept the full gate for R15 and integration.
- I5: Jev re-scored proofs 0.23–0.67; tightened R3 (timestamps), R5 (20-query golden table) and R14 (narrowed to colours, three samples per view and mode). Post-convergence amendments from Phase 0 Task 11 (not new loop passes): the guard and T4 also protect desktop-gpui/src/automation, scripts/probe-helpers and scripts/ocr.swift; R1's delayed capture is 1 s so the capturing state is observable without flake; R8 uses ⌘L (Tauri parity — plain L types into the filter; ruling R-n); R13 proven through a fixture-fed SMAppService seam (ruling R-o).

## Open questions
