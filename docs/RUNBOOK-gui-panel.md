# Runbook — GUI panel (U1 / U2 / U4 display verification)

The linking panel is a **no-build, zero-npm-dependency** vanilla frontend in `dist/`
(keeps S2 honest; nothing to phone home) plus a Rust command-contract backend
(`src-tauri/src/panel.rs`) and the Tauri shell (`desktop/`). The interaction logic
and the accessibility/structure contract are proven headlessly; what needs a real
display + the Tauri webview is listed under "On a real Mac".

## What is already proven (no GUI needed)
- **View-state logic:** 25 `node --test` cases. `dist/panel-state.test.mjs` covers
  the panel: open() renders synchronously without capture data (U1 render-first),
  open→confirm is exactly two actions (U2), backlinks surface on ready (U5), every
  failure carries a non-empty explanation (U3), and permission onboarding.
  `dist/browse-state.test.mjs` covers the link browser.
- **Command contract** (`panel.rs`): `capture_current` surfaces a resource's
  backlinks (U5) and turns any capture failure into an explained message (U3); other
  commands keep working while a capture is in flight; a captured link is validated
  before it is stored; `open_link` re-validates with the shared link policy; plus the
  link browser, settings (stats/export/reset) and capability commands.
- **Markup/style contract** (`panel.rs`): `dist/index.html` carries `role="dialog"`,
  an accessible name, an `aria-live` status, a `role="alert"` error region, a single
  primary Copy action with an Enter shortcut, and the permission onboarding screen;
  `dist/panel.css` has a `:focus-visible` ring and honours `prefers-reduced-motion`.

Run them: `make test-js` and `cargo test --manifest-path src-tauri/Cargo.toml panel`.

## Recommended: run the bundled app (so permission grants persist)

`make run` (`cargo run`) launches a bare binary whose path changes per build, so macOS
re-prompts for Accessibility/Automation each time. For a stable experience:

```sh
make bundle     # -> desktop/target/bundle/Ubilink.app (stable self-signed identity, fixed path)
open desktop/target/bundle/Ubilink.app
```

Grant Accessibility + Automation to **Ubilink.app** once; re-running `make bundle`
rebuilds in place, so the grant sticks. (Optionally drag `Ubilink.app` to /Applications
for an even more stable location.)

## On a real Mac

1. **The app.** The `desktop/` crate registers `#[tauri::command]` wrappers over
   `PanelBackend` (capture, link browser, settings with export/import, the
   app-adapter manager, clipboard, open, permissions) and three global shortcuts:
   **Cmd+Shift+K** (capture panel), **Cmd+Shift+L** (link browser) and
   **Cmd+Shift+,** (settings). It is a separate crate so ubilink-core stays
   Tauri-free (S2).

   On launch the panel shows a **permission onboarding screen** (S4): it checks
   Accessibility (`AXIsProcessTrusted`) and Automation (System Events probe), shows
   ✓/✗ per item, and offers "Open Settings" (jumps to the right privacy pane) +
   "Re-check". Grant both, Re-check → the capture view appears.

   The capture flow is **hotkey-driven and inverted**: pressing Cmd+Shift+K captures
   whatever app was frontmost *before* the panel appears. The panel paints first,
   then the capture fills in, and it re-captures on every press, so it links the app
   you were in, not the panel itself. Esc dismisses.
2. **U1 — p99 < 200 ms (p50 < 80 ms) hotkey-to-visible.** Measure hotkey→first-paint
   over ≥100 invocations with the `UBILINK_PERF` harness (`desktop/src/main.rs`,
   `dist/panel.js`). Confirm the spinner paints before capture resolves.
3. **U2 — ≤ 2 actions.** From a focused resource in another app: hotkey, then Enter.
   Confirm the link is on the clipboard and the panel dismissed — two actions, no more.
4. **U4 — accessibility.** Drive the whole flow with the keyboard only (no mouse).
   Run VoiceOver: confirm the dialog, status, error, and backlinks are announced.
   Check text contrast meets WCAG AA on the real display.
5. **O2 — in-app capability disclosure.** The `capabilities` command returns the
   matrix derived from the loaded adapters (tested), but `dist/panel.js` does not
   call it yet. The Settings adapter manager lists the available app adapters, not
   the per-platform matrix. The published half of O2
   ([`CAPABILITIES.md`](CAPABILITIES.md)) is generated and drift-guarded.

## Recording the result
U1, U2 and U4 are recorded as SATISFIED in
`.manifold/linking-manifesto-app.verify.json` from live runs on a real Mac (U1:
n=100, p50 27 ms / p99 90 ms; U2: hotkey + Enter; U4: keyboard-only + VoiceOver
pass, AA contrast). Re-run steps 2–4 after changes to the hotkey path, the panel
markup or its styles. Change a constraint's status only on observed evidence, never
from the headless build.
