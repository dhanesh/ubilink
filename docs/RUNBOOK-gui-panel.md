# Runbook — GUI panel (U1 / U2 / U4 ENV-blocked verification)

The linking panel is scaffolded as a **no-build, zero-npm-dependency** vanilla
frontend in `dist/` (keeps S2 honest; nothing to phone home) plus a Rust
command-contract backend (`src-tauri/src/panel.rs`). The interaction logic and the
accessibility/structure are proven headlessly; what needs a real display + the Tauri
webview is listed under "Manual steps".

## What is already proven (no GUI needed)
- **View-state logic** (`dist/panel-state.mjs`): 8 `node --test` cases — open()
  renders synchronously without capture data (U1 render-first), open→confirm is
  exactly two actions (U2), backlinks surface on ready (U5), every failure carries a
  non-empty explanation (U3).
- **Command contract** (`panel.rs`): `capture_current` surfaces a resource's
  backlinks (U5) and turns any capture failure into an explained message (U3) — 2
  Rust tests over the integrated `HookyService`.
- **Markup/style contract** (`panel.rs`): `dist/index.html` carries `role="dialog"`,
  an accessible name, an `aria-live` status, a `role="alert"` error region, a single
  primary Copy action with an Enter shortcut; `dist/panel.css` has a `:focus-visible`
  ring and honours `prefers-reduced-motion` — 3 Rust tests.

Run them: `node --test 'dist/*.test.mjs'` and `cargo test panel`.

## Manual steps (a Mac with a desktop session) — what stays ENV-blocked

1. **Wire the Tauri command layer.** Add the `tauri` crate + a binary that registers
   `#[tauri::command]` wrappers calling `PanelBackend` (`capture_current`,
   `copy_to_clipboard`, `dismiss_panel`, `open_link`) and a global shortcut that
   shows the (already-configured, hidden) `panel` window from `tauri.conf.json`.
   *Deliberately not added headlessly* — it pulls a webview/runtime stack the core
   avoids, and can't be exercised without a display.
2. **U1 — p99 < 200 ms (p50 < 80 ms) hotkey-to-visible.** With the panel running,
   measure hotkey→first-paint over ≥100 invocations (e.g. timestamp in the global
   shortcut handler vs. a `requestAnimationFrame` in `panel.js`). Confirm the spinner
   paints before capture resolves (the render-first contract). Record the percentiles.
3. **U2 — ≤ 2 actions.** From a focused resource in another app: hotkey, then Enter.
   Confirm the link is on the clipboard and the panel dismissed — two actions, no more.
4. **U4 — accessibility.** Drive the whole flow with the keyboard only (no mouse).
   Run VoiceOver: confirm the dialog, status, error, and backlinks are announced.
   Check text contrast meets WCAG AA (the tokens in `panel.css` are chosen for it,
   but verify on the real display).

## Recording the result
Each measured item flips its constraint only on observed evidence:
- U1 → SATISFIED when the p99/p50 numbers are recorded green.
- U2 → SATISFIED when the 2-action flow is demonstrated end-to-end.
- U4 → SATISFIED when the keyboard-only + VoiceOver + contrast pass is done.
Update `.manifold/linking-manifesto-app.verify.json` only then — never from the
headless build, which keeps these at PARTIAL (UI built + contract-tested, real-display
verification pending).
