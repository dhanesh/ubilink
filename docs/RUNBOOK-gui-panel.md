# Runbook — GUI panel (U1 / U2 / U4 display verification)

The panel is the native GPUI app in `desktop-gpui/` (binary `ubilink`, Ubilink.app,
`org.dhanesh.ubilink`; D22, D24). Its logic lives in the GUI-free `app/` command layer
and in ubilink-core (`src-tauri/`), both tested headlessly; the views are proven on the
running app by the GPUI probes. What still needs a person at a real Mac is listed under
"On a real Mac".

For the unattended GPUI probe runs (factory-conductor), see
`docs/RUNBOOK-phase1-unattended.md` for the host preflight.

## What is already proven (no person needed)
- **Command contract** (`src-tauri/src/panel.rs`, `app/`): `capture_current` surfaces a
  resource's backlinks (U5) and turns any capture failure into an explained message that
  says what to do (U3); other commands keep working while a capture is in flight; a
  captured link is validated before it is stored; `open_link` re-validates with the
  shared link policy; plus the link browser, settings (stats/export/import/reset) and
  capability commands. `make test`.
- **Views on the running app** (`desktop-gpui/probes/`, `scripts/probe.py`): every
  view's behaviour, its accessibility tree (roles, names, focus) and its colours are
  driven and read through an automation build that release builds never contain (D23):
  `python3 scripts/probe.py doctor`, then `python3 scripts/probe.py run <scenario>…`
  (`make probe` runs a smoke set). They need a desktop session, so they are not part of
  `make verify`; `make verify` does run the GPUI unit tests (`make test-gpui`) and
  `release-clean`.
- **SHA-bound evidence per feature:** the verify skill,
  `desktop-gpui/.claude/skills/verify-ubilink/SKILL.md`.

Probes and the verify skill always launch isolated instances with their own
`UBILINK_DATA_DIR`. Do the same when you try a build by hand.

## Running the bundled app (so permission grants persist)

macOS ties Automation grants to the app's signature and path, so use the bundle, not
`cargo run`:

```sh
make bundle     # -> desktop-gpui/bundle/Ubilink.app (stable self-signed identity, fixed path)
make run        # make bundle, then open it
```

**`make run` uses your real data dir** (`~/Library/Application Support/org.dhanesh.ubilink`,
the same graph the released app uses): `open` launches through LaunchServices, which
passes no environment, so `UBILINK_DATA_DIR` cannot redirect it. To try a build against
throwaway data, quit any running Ubilink and start the bundled binary from a shell
instead:

```sh
pgrep -x ubilink   # must print nothing: a running copy owns the hotkeys
d="$(mktemp -d)"
UBILINK_DATA_DIR="$d" UBILINK_INSTANCE_SOCKET="$d/instance.sock" \
  desktop-gpui/bundle/Ubilink.app/Contents/MacOS/ubilink
```

`UBILINK_INSTANCE_SOCKET` gives it its own single-instance socket; without it, a launch
while another Ubilink runs only asks that one to show its panel and exits.

Grant Automation to **Ubilink.app** once; re-running `make bundle` rebuilds in place, so
the grant sticks. (Optionally drag `Ubilink.app` to /Applications for an even more
stable location.) `make bundle` imports a self-signed signing identity into your
keychain the first time.

## On a real Mac

1. **The app.** A menu-bar utility (no Dock icon) with a tray item and three global
   shortcuts: **Cmd+Shift+K** (capture panel), **Cmd+Shift+L** (link browser) and
   **Cmd+Shift+,** (settings). It is a separate crate so ubilink-core stays GUI-free
   (S2).

   Until Automation is granted, a capture routes to the **permission onboarding** view
   (S4): it shows the Automation status, offers to open the right System Settings pane
   and re-checks. Accessibility is not requested: no built-in adapter needs it.

   The capture flow is **hotkey-driven and inverted**: pressing Cmd+Shift+K captures
   whatever app was frontmost *before* the panel appears. The panel never takes the
   frontmost app (a non-activating panel), paints first, then the capture fills in, and
   it re-captures on every press, so it links the app you were in, not the panel
   itself. Esc dismisses. Live capture per app: `docs/RUNBOOK-live-capture.md`.
2. **U1 — p99 < 200 ms (p50 < 80 ms) hotkey-to-visible.** `make budget` measures the
   release app (hotkey latency, idle CPU, footprint, binary size) and writes
   `docs/PERFORMANCE.md`. Run it on a quiet host; also press the real hotkey and confirm
   the panel paints before the capture resolves.
3. **U2 — ≤ 2 actions.** From a focused resource in another app: hotkey, then Enter.
   Confirm the link is on the clipboard and the panel dismissed — two actions, no more.
4. **U4 — accessibility.** Drive the whole flow with the keyboard only (no mouse).
   Run VoiceOver: confirm the panel, the capture status, errors and backlinks are
   announced (the probes read the same AccessKit tree, but not what VoiceOver says).
   Check text contrast meets WCAG AA on the real display, in light and dark mode.
5. **Host-only features the probes stub out (O4):** turn **Launch at login** on and off
   in Settings and confirm System Settings › General › Login Items follows
   (SMAppService). **Export** writes `ubilink-export-<time>.json` into Downloads (or
   home), with no dialog, and Finder opens with the file selected; the hint line names
   the path. **Import**: the open dialog must come to the front, above the panel. Pick
   the exported file: the counts come back. Then press Import again and cancel with
   Escape: Settings is still open and takes Tab and Escape. Pressing Import while its
   dialog is open does nothing; it never stacks a second dialog. No probe can do this:
   automation builds never open host dialogs (O4).
6. **O2 — in-app capability disclosure.** The `capabilities` command returns the matrix
   derived from the loaded adapters (tested), but no view shows it yet. The Settings
   adapter list shows the available app adapters, not the per-platform matrix. The
   published half of O2 ([`CAPABILITIES.md`](CAPABILITIES.md)) is generated and
   drift-guarded.

## Recording the result
U1, U2 and U4 are recorded as SATISFIED in
`.manifold/linking-manifesto-app.verify.json` from live runs on a real Mac (U1:
n=100, p50 27 ms / p99 90 ms; U2: hotkey + Enter; U4: keyboard-only + VoiceOver
pass, AA contrast). Those runs were on the earlier Tauri panel (D22); the GPUI app's
budget is in `docs/PERFORMANCE.md`. Re-run steps 2–5 on the GPUI app after changes to
the hotkey path, a view, or the theme. Change a constraint's status only on observed
evidence, never from the headless build.
