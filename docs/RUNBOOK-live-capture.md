# Runbook — verifying live macOS app capture (B1 / S4 ENV-blocked seam)

Capture is declarative. The desktop asks System Events for the frontmost app's name
(`FRONTMOST_APP_SCRIPT`), picks that app's adapter from the `AdapterRegistry`
(built-ins in `src-tauri/src/adapters.default.json` plus your `adapters.json`), and
`ConfiguredAdapter::capture` runs the adapter's AppleScript with `osascript -e`,
then renders its `uri`/`name` templates. Every `osascript` call (frontmost-app
detection, the permission probe and each adapter) goes through `OsaScriptRunner`,
bounded by `OSASCRIPT_TIMEOUT` (15 s) in `src-tauri/src/live_capture.rs`.

All of that is unit-tested with a fake `CommandRunner` except the **real
`osascript` call against a live app**, which needs a Mac, the app, and a TCC
Automation grant. This runbook confirms that seam.

## What is already proven (no Mac needed)
- Every shipped adapter's templates render, and browser (`url<TAB>title`), Finder
  (POSIX path → `file://…`) and Bear output parse into a `ResourceIdentity`
  (`adapters_config.rs` tests).
- Empty or unexpected replies and runner errors degrade to an explained
  `CaptureFailure::AdapterError` (U3 — never silent, never a panic).
- The S4 permission gate blocks capture **before** the runner is called
  (`adapter.rs::permission_gate_blocks_capture_with_explanation_not_silence`).
- The permission probe maps osascript success → `Granted`, `-1743` → `Denied`,
  anything else → `NotDetermined`.
- `run_bounded` kills and reaps a stalled `osascript` at the deadline, and output on
  both pipes cannot deadlock the wait (`live_capture.rs` tests); a stalled adapter's
  error names the app (`a_stalled_app_times_out_with_an_explanation_naming_it`).
- The backend lock is not held while an app is being captured, and the captured link
  is still validated before it is stored (`panel.rs` tests).

## Manual steps (a Mac with a desktop session)

1. **Launch the app and grant access.** Use the bundled app so grants persist
   (`make bundle`, see [`RUNBOOK-gui-panel.md`](RUNBOOK-gui-panel.md)). The onboarding
   screen checks Accessibility and Automation. The **first** capture from each app
   also raises a macOS Automation prompt for that app. `osascript` waits for your
   answer, but only for 15 s. If the timeout expires first, the panel says
   *"…the app didn't respond within 15 seconds. If macOS is showing a permission
   prompt, answer it, then try again."* Answer the prompt and press the hotkey again.
2. **Safari:** open any page (e.g. `https://example.com`), press **Cmd+Shift+K**.
   Expect the panel to show the page title and URL; Enter copies the link.
3. **Other built-ins:** repeat with a Chrome (or Arc/Brave/Edge/Vivaldi/Opera) tab,
   a Finder window, a Bear note (needs Accessibility: the adapter clicks
   *Note › Copy Link*) and a selected Things3 to-do.
4. **Seam check without the panel (optional):** run an adapter's script directly.
   The grant belongs to your terminal here, not to `Ubilink.app`:
   ```sh
   osascript -e 'tell application "Safari" to return (URL of current tab of front window) & tab & (name of current tab of front window)'
   ```
   Expect `https://…<TAB><title>`. Error `-1743` means the Automation grant is missing.

What the panel's errors mean:
- *"Linking from “X” isn't supported yet."* No adapter matches the frontmost app's
  name ([`SUPPORTED-APPS.md`](SUPPORTED-APPS.md) shows how to add one).
- *"Linking from “X” is turned off in Settings."* A custom adapter you disabled.
- *"X returned no active resource — open something there and try again."* The
  script ran but returned nothing (no front window, tab or selection).

## Recording the result
On success, this closes the live-app residual of **B1**. Update
`.manifold/linking-manifesto-app.verify.json` (B1 → SATISFIED, drop the
`live-app capture` residual) **only after** observing the green run — never from
the headless build, which deliberately keeps it ENV-blocked.
