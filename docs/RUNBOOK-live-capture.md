# Runbook — verifying live macOS app capture (B1 / S4 ENV-blocked seam)

The live-app capture logic (`src-tauri/src/live_capture.rs`) is proven headlessly
by 10 unit tests using a fake `CommandRunner`. The **only** part that cannot be
verified without a real desktop is the single `osascript` invocation in
`OsaScriptRunner` — it needs a live app and a TCC automation grant. This runbook
confirms that one seam on a Mac.

## What is already proven (no Mac needed)
- AppleScript selection per app (Safari / Chrome / Finder) names the right app.
- `url \t title` and Finder-path parsing → `ResourceIdentity` (uri/name/path).
- Empty / title-only / runner-error replies degrade to an explained
  `CaptureFailure::AdapterError` (U3 — never silent, never a panic).
- The S4 permission gate blocks capture **before** the runner is ever called.
- The permission probe maps osascript success → `Granted`, `-1743` → `Denied`,
  anything else → `NotDetermined`.

## Manual steps (a Mac with a desktop session)

1. **Grant automation access.** First run will prompt; or pre-grant in
   *System Settings › Privacy & Security › Automation* (and *Accessibility* if the
   runner is a wrapped binary). Without the grant the probe returns `Denied`.
2. **Open Safari** to any page (e.g. `https://example.com`).
3. **Run the ignored live test:**
   ```sh
   cargo test -p ubilink-core -- --ignored live_capture_against_real_safari
   ```
4. **Expect:** the test prints `captured live: uri=https://… name=…` and passes.
   - Fails with a permission assertion → the TCC grant is missing (step 1).
   - Fails with "is Safari open" → no front window/tab; open one and retry.
5. **Chrome / Finder:** swap `AppKind::Safari` for `AppKind::Chrome` (tab open) or
   `AppKind::Finder` (a Finder window with a target folder) to confirm those seams.

## Recording the result
On success, this closes the live-app residual of **B1** and confirms the **S4**
probe end-to-end on a real desktop. Update
`.manifold/linking-manifesto-app.verify.json` (B1 → SATISFIED, drop the
`live-app capture` residual) **only after** observing the green run — never from
the headless build, which deliberately keeps it ENV-blocked.
