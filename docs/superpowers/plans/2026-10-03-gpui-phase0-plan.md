# Plan: GPUI Phase 0 — native shell, automation channel and verify skill

Spec: `docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md` (binding authority). Phase 1 spec, which this phase must make runnable: `docs/superpowers/specs/2026-10-03-gpui-phase1-port.md`.

## Execution order

Tasks keep their spec numbers. Run them in this order (dependency-safe, sequential):
1, 2, 3, 12, 4, 13, 6, 8, (re-run Task 4's shell criterion), 7, 5, 9, 10, 11, 16, 17, 18, 14, 15, 19.

## Global Constraints

- Toolchain Rust 1.98.x; GPUI stack exactly: `gpui-kit = "=0.7.0"` (re-exports `gpui-pre =0.3.7`; never add crates.io `gpui` 0.2.2), `tray-icon = "0.26"`, `global-hotkey = "0.8"`, `objc2 = "0.6"`, `objc2-app-kit = "0.3"`, `objc2-service-management = "0.3"`, `interprocess = "2.4"`, plus serde/serde_json already used in the repo. No other new crates. Commit every Cargo.lock.
- `ubilink-core` (`src-tauri/`) gains no GUI or network dependency; its `no_network_deps` guard keeps passing.
- The Tauri app (`desktop/`) and `dist/` keep passing `make test-desktop` and `make test-js` throughout.
- PROTECTED PATHS: `.github/**`, `Makefile`, `.claude/settings*.json`. A repo hook denies edits to them in auto mode. Implementers MUST NOT modify them by any means (no Edit/Write, no sed/awk/cat/python/git apply in Bash). Write the intended change as a unified diff at `.superpowers/sdd/2026-10-03-gpui-phase0-plan/patches/T<N>-<name>.patch`; the controller takes it to the user.
- The automation channel compiles only under the `automation` cargo feature; release builds carry no socket path or verb names.
- No real side effects from probes: opens go to the sink, dialogs take fixture paths, GUI probes hold the desktop lock, automation instances skip real hotkeys.
- Containers, if any, run through Colima (none expected).
- Follow the repo's style: comments explain *why* and cite decisions/constraints (see desktop/src/main.rs, DECISIONS.md).

## Task 1 — The toolchain must be Rust 1.98.x with edition 2024 available, set in `rust-toolchain.toml` and every workflow's toolchain input, with core, registry and desktop crates passing clippy on it

- Satisfies: R1
- Where: rust-toolchain.toml, .github/workflows/
- Verify:
  - [ ] `rustc --version` reports 1.98 inside the repo and no workflow still names 1.92. (cmd: `{python} scripts/probe.py toolchain --expect 1.98`)
  - [ ] The existing gate passes on the new toolchain. (cmd: `make verify`)
- Execution note: edit `rust-toolchain.toml` (channel 1.98.x; keep components and both targets) and fix any clippy/fmt fallout in core, registry and desktop. The `.github/workflows/*.yml` toolchain inputs are PROTECTED (see Global Constraints): write `.superpowers/sdd/<plan>/patches/T1-workflows.patch` instead. `scripts/probe.py toolchain --expect 1.98` does not exist yet — create `scripts/probe.py` as a stdlib-only Python CLI with an argparse subcommand table and implement only `toolchain` (checks `rustc --version` and that no file under .github/workflows names the old version). Later tasks add subcommands.

## Task 2 — A new crate `desktop-gpui/` must build `ubilink-next.app` (bundle id `org.ubilink.next`, stable dev signing identity) from the pinned stack with every objc2 and gpui-kit feature the four views need enabled, a stripped LTO release profile, and its Cargo.lock committed

- Satisfies: R2
- Where: desktop-gpui/, scripts/bundle-next.sh
- Verify:
  - [ ] The bundled app builds locked and its Info.plist carries org.ubilink.next. (cmd: `bash scripts/bundle-next.sh dev`)
- Execution note: binary `ubilink-next`; deps exactly the pinned stack (Global Constraints) with the objc2-app-kit / objc2-service-management / gpui-kit features the four views will need (NSPasteboard, NSWorkspace, NSEvent, NSScreen, NSStatusBar-adjacent, SMAppService; gpui-kit list/virtual_list/input/switch/setting/dialog/notification/theme). `scripts/bundle-next.sh dev` reuses the identity logic of `scripts/bundle-macos.sh` (read it) to produce `desktop-gpui/target/.../ubilink-next.app` with LSUIElement=true. A `main.rs` that opens nothing yet is fine.

## Task 3 — The 21 command bodies in `desktop/src/main.rs` must move into a GUI-free `ubilink-app` crate that both shells call, with no tauri or gpui crate in its dependency tree

- Satisfies: R3
- Where: app/, desktop/src/main.rs
- Verify:
  - [ ] The app crate's tests pass. (cmd: `cargo test --locked --manifest-path app/Cargo.toml`)
  - [ ] The dependency guard finds no tauri or gpui crate under ubilink-app. (cmd: `{python} scripts/probe.py deps-clean app/Cargo.toml`)
  - [ ] The Tauri shell's tests still pass after the move. (cmd: `make test-desktop`)
- Execution note: a pure move. Create `app/` crate `ubilink-app` depending on ubilink-core; move the command bodies (not the `#[tauri::command]` attributes) so each Tauri command becomes a thin wrapper. Window/tauri-specific commands (resize_panel, dismiss_panel, restart_app, retry_hotkeys) stay in the shell if they need AppHandle — record which in the report. Add `deps-clean` to scripts/probe.py (`cargo tree --manifest-path <p> --prefix none` contains no tauri*/gpui* crate).

## Task 4 — The GPUI shell must show a tray item with a menu, register ⌘⇧K, ⌘⇧L and ⌘⇧, globally, open a `WindowKind::PopUp` panel with a blurred background on the display under the cursor, and run with the accessory activation policy, all in one process

- Satisfies: R4
- Where: desktop-gpui/src/shell/
- Verify:
  - [ ] The shell probe sees tray_present, three hotkeys registered, window kind popup, background blurred and activation accessory. (cmd: `{python} scripts/probe.py run shell`)
  - [ ] The maintainer presses each real hotkey with the cursor on a second display and sees the panel open there, before and after one sleep/wake cycle.
- Execution note: tray-icon + global-hotkey created on the main thread inside `Application::run`, events polled from a GPUI task (spawned timer loop). Accessory policy via objc2-app-kit `NSApplication::setActivationPolicy` (gpui-pre 0.3.7 lacks ActivationPolicy). Cursor display via `NSEvent::mouseLocation` vs `cx.displays()` bounds. If tray-icon or global-hotkey cannot coexist, apply the spec's D5 fallback and say so. The `probe.py run shell` criterion cannot run until Tasks 6 and 8 exist: implement the shell so its state (tray_present, hotkeys_registered, window_kind, background, activation_policy) is held in a serialisable struct the automation `state` verb will return; the controller re-runs this criterion after Task 8.

## Task 5 — The shell panel must reach first frame from a hotkey with p99 < 200 ms and p50 < 80 ms over 50 triggers, and must hide on loss of key status in capture and browser while staying open in onboarding and settings

- Satisfies: R5
- Where: desktop-gpui/src/shell/
- Verify:
  - [ ] The panel-hotkey and panel-dismiss probes exit 0 against the stub views. (cmd: `{python} scripts/probe.py run panel-hotkey panel-dismiss`)
- Execution note: runs after Task 8 (needs the probe runner). Add the `panel-hotkey` and `panel-dismiss` scenarios under desktop-gpui/probes/ and the metrics ring buffer (timed at frame present).

## Task 6 — An automation channel compiled only under the `automation` feature must serve open_view, key, state, metrics, trigger_hotkey, resign_key, set_appearance, load_fixture, window_id, a11y (activating and dumping GPUI's AccessKit tree in-process) and opened (the opener sink) over a uid-checked 0600 Unix socket, separate from the single-instance socket

- Satisfies: R6
- Where: desktop-gpui/src/automation/
- Verify:
  - [ ] The release build contains no automation socket path or verb names. (cmd: `{python} scripts/probe.py release-clean`)
  - [ ] Headless test `automation_rejects_unknown_verb` passes. (cmd: `cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation automation_rejects_unknown_verb`)
- Execution note: `#[cfg(feature = "automation")]` module, socket at `$TMPDIR/ubilink-<uid>/<instance>/auto.sock` (dir 0700, socket 0600, getpeereid uid check), newline-delimited JSON request/response, verbs exactly as listed; unknown verb → error. `a11y` activates the AccessKit tree in-process (send accessibilityChildren to the window's NSView, wait one frame, return `Window::debug_a11y_tree_json()`). `opened` returns the opener sink (automation builds route opens there). Add `release-clean` to probe.py: build release without the feature and assert `strings` finds no `auto.sock` and no verb names.

## Task 7 — A spike must show that gpui-kit's list, input, switch and dialog components emit accessibility nodes with their visible text once the tree is activated in-process

- Satisfies: R7
- Where: desktop-gpui/src/automation/
- Verify:
  - [ ] The a11y spike probe sees nodes with visible text for list, input, switch and dialog. (cmd: `{python} scripts/probe.py run a11y-spike`)
- Execution note: runs after Task 8. A hidden spike view (automation builds only) containing a gpui-kit list, input, switch and dialog; the `a11y-spike` probe asserts each emits a node with its visible text. If a component emits nothing, report it precisely — Phase 1 proofs depend on this.

## Task 8 — `scripts/probe.py` must launch an isolated instance (own data dir, socket, fixtures, fixture file paths and no real hotkeys), run scenario files with the steps see, ocr, pixel, pasteboard, graph, file, winframe, warp_cursor and activate_other, restart an instance on request, run two side by side under a desktop lock, and seed each build from a warm target directory with an APFS clone

- Satisfies: R8
- Where: scripts/probe.py, scripts/ocr.swift
- Verify:
  - [ ] Two instances run side by side under the lock and each answers `state`, and an `opened` call reports the sink without launching anything. (cmd: `{python} scripts/probe.py run shell --instances 2`)
- Execution note: probe.py grows the instance runner: per-instance data dir + socket + fixtures under a temp root, `UBILINK_DATA_DIR`/`UBILINK_INSTANCE`/`UBILINK_SKIP_HOTKEYS=1` env for the automation build, scenario JSON format (document it in desktop-gpui/probes/README.md), the listed steps, a desktop flock (`~/.cache/ubilink-probe/desktop.lock`), restart, `--instances N`, and a warm target dir seeded with `cp -c` (APFS clone). `scripts/ocr.swift` is compiled once with swiftc into the cache dir. After this task the controller re-runs Task 4's shell criterion.

## Task 9 — `probe.py doctor` must report Screen Recording access, display awake, display count, a compiled OCR helper and an active caffeinate assertion, and exit non-zero when any is missing

- Satisfies: R9
- Where: scripts/probe.py
- Verify:
  - [ ] Doctor passes on the maintainer's Mac. (cmd: `{python} scripts/probe.py doctor`)
- Execution note: `doctor` checks: Screen Recording (CGPreflightScreenCaptureAccess via a tiny swift helper), display awake (CGDisplayIsAsleep), display count, OCR helper compiled, caffeinate assertion (pmset -g assertions). Non-zero exit naming each missing item.

## Task 10 — Fixtures must exist for a 5,000-link graph, a 20-query golden ranking table, a 3-backlink timestamped seed with a decoy, permission states, a delayed capture, a failed capture, and an export path

- Satisfies: R10
- Where: desktop-gpui/probes/fixtures/
- Verify:
  - [ ] The fixture check finds every fixture and validates the 5,000-link graph and the 20-query table. (cmd: `{python} scripts/probe.py fixtures --check`)
- Execution note: fixtures are data files plus a `probe.py fixtures --check` validator. The 5,000-link graph is generated deterministically (seeded) by probe.py and checked in as JSON in the core's export format. The 20-query golden table lists query → expected top-3 URIs, with decoys.

## Task 11 — Probe scenarios for all 15 Phase 1 checks must exist under `desktop-gpui/probes/`, each asserting view-specific accessibility content and each failing against the stub views

- Satisfies: R11
- Where: desktop-gpui/probes/
- Verify:
  - [ ] All 15 Phase 1 scenarios are listed and every one fails against the stubs. (cmd: `{python} scripts/probe.py list --expect 15 --expect-failing 15`)
- Execution note: 15 scenarios named exactly: capture-view, copy-link, backlinks, link-browser, link-browser-filter, browser-select, browser-drill, link-marking, onboarding, settings-adapters, graph-roundtrip, graph-reset, launch-at-login, theming, performance — each asserting the Phase 1 spec's [proof:] for its requirement (read docs/superpowers/specs/2026-10-03-gpui-phase1-port.md). `probe.py list --expect 15 --expect-failing 15` runs them all against the stubs.

## Task 12 — A converter must turn `design/ubilink-tokens.json` into gpui-kit light and dark theme configs that the shell loads at start

- Satisfies: R12
- Where: desktop-gpui/src/theme/
- Verify:
  - [ ] Headless test `theme_tokens_convert` passes. (cmd: `cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation theme_tokens_convert`)
- Execution note: converter at build time or start-up from design/ubilink-tokens.json → gpui-kit ThemeConfig (light + dark); test `theme_tokens_convert` asserts concrete token values land in the config.

## Task 13 — A router table must register four stub views (capture, browser, onboarding, settings), each in its own file under `desktop-gpui/src/views/` exposing a serialisable view-state for the `state` verb

- Satisfies: R13
- Where: desktop-gpui/src/views/
- Verify:
  - [ ] Headless test `router_registers_four_views` passes. (cmd: `cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation router_registers_four_views`)
- Execution note: the router table is the single place views register; each stub view lives in its own file and implements a `ViewState` serialisable for the `state` verb. Test `router_registers_four_views`.

## Task 14 — A verify skill at `desktop-gpui/.claude/skills/verify-ubilink` must pass `forge lint` and be run live once on the shell feature, with its evidence recorded at the driven commit

- Satisfies: R14
- Where: desktop-gpui/.claude/skills/verify-ubilink
- Verify:
  - [ ] `forge lint` prints LINT_RESULT: PASS for the verify skill, and `.verify/` holds an evidence.json for the shell feature naming the driven commit.
- Execution note: run the verification-skill-forge skill (read `~/.agents/skills/verification-skill-forge/SKILL.md`; its helper is `~/.agents/skills/verification-skill-forge/assets/forge.py`) in forge mode with app root `desktop-gpui`, app name `ubilink`. Launch/Drive/Evidence use scripts/probe.py and the automation verbs. Prove it live on the `shell` feature.

## Task 15 — The verify skill's feature map must contain one entry for each of the 14 Phase 1 feature ids plus `shell`

- Satisfies: R15
- Where: desktop-gpui/.claude/skills/verify-ubilink/features/
- Verify:
  - [ ] `forge lint` passes and `features/README.md` lists 15 entries matching the 14 Phase 1 feature ids plus shell.
- Execution note: feature ids exactly: capture-view, copy-link, backlinks, link-browser, browser-navigation, link-marking, onboarding, settings-adapters, graph-data, launch-at-login, theming, resource-budget, panel-hotkey, panel-dismiss, shell. Phase 1 features stay unproven.

## Task 16 — `make verify` must include a `test-gpui` target and `probe.py release-clean`, and CI must run it on macos-latest

- Satisfies: R16
- Where: Makefile, .github/workflows/ci.yml
- Verify:
  - [ ] The gate runs the GPUI tests and the release-clean check. (cmd: `make verify`)
  - [ ] CI is green on the pull request on macos-latest.
- Execution note: Makefile and .github/workflows/ci.yml are PROTECTED: write `patches/T16-makefile-ci.patch` for the controller to bring to the user.

## Task 17 — DECISIONS.md must record D22 (GPUI shell, superseding D8 and D11, with the gpui-pre zed revision 1a28cff) and D23 (automation channel and its security rules)

- Satisfies: R17
- Where: DECISIONS.md
- Verify:
  - [ ] DECISIONS.md has D22 and D23 headings. (cmd: `grep -c '^## D2[23] ' DECISIONS.md`)
- Execution note: follow the existing DECISIONS.md entry style (read D20, D21).

## Task 18 — The Manifold model must name the GPUI shell and probe evidence for U1–U5 so `make verify-spec` passes

- Satisfies: R18
- Where: .manifold/
- Verify:
  - [ ] The Manifold spec validates. (cmd: `make verify-spec`)
- Execution note: edit .manifold/linking-manifesto-app.{json,md} so U1–U5 evidence names the GPUI probes; `make verify-spec` must pass (needs the `manifold` CLI, installed).

## Task 19 — The final Phase 0 commit must be tagged `phase0-done`, and every `phase0-done` in the Phase 1 spec must be replaced by that commit's literal sha before the Phase 1 grant is written

- Satisfies: R19
- Where: docs/superpowers/specs/2026-10-03-gpui-phase1-port.md
- Verify:
  - [ ] The tag exists and the Phase 1 spec no longer names it. (cmd: `git rev-parse --verify phase0-done`)
  - [ ] `grep -c phase0-done docs/superpowers/specs/2026-10-03-gpui-phase1-port.md` prints 0, and the Phase 1 spec still passes `spec_lint.py --unattended`.
- Execution note: controller-run at the very end.

