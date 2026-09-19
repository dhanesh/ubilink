// Satisfies: U1 (panel visible INSTANTLY — open() resolves synchronously to a
// rendered state; capture data arrives as a SEPARATE later event, never blocking
// the first paint), U2 (link obtained in <=2 user actions: open then confirm), U3
// (every failure carries an explanation — phase 'error' always has a message), U5
// (a ready resource surfaces its backlinks). See .manifold/linking-manifesto-app.md
//
// The panel's view-state is a pure reducer with NO dependencies and NO I/O, so the
// whole interaction model is testable headlessly with `node --test` (zero npm deps,
// keeping S2 honest). panel.js is the thin DOM/Tauri shell over this.

/** The instant first-paint state: the panel is up, capture is in flight (U1). */
export const initialCapturing = Object.freeze({ phase: 'capturing' });

/**
 * Pure transition. `state` is the current view-model; `event` is what happened.
 * Returns the next view-model. Unknown events return the state unchanged.
 *
 * Events:
 *   {type:'open'}                          -> 'capturing'  (synchronous, U1)
 *   {type:'captured', resource, backlinks} -> 'ready'      (data filled in, U5)
 *   {type:'failed', message}               -> 'error'      (explained, U3)
 *   {type:'reset'}                         -> 'idle'
 */
export function reduce(state, event) {
  switch (event.type) {
    case 'open':
      // U1: going visible does NOT wait for capture — we render 'capturing' now.
      return { phase: 'capturing' };
    case 'captured':
      return {
        phase: 'ready',
        resource: event.resource,
        backlinks: Array.isArray(event.backlinks) ? event.backlinks : [],
      };
    case 'failed':
      // U3: never silent — coerce a missing/empty message to a generic explanation.
      return {
        phase: 'error',
        message:
          event.message && String(event.message).trim().length > 0
            ? String(event.message)
            : "ubilink couldn't capture a link, and isn't sure why. Try again.",
      };
    case 'reset':
      return { phase: 'idle' };
    default:
      return state;
  }
}

/** U2: the confirm action is available only once a resource is ready. */
export function canConfirm(state) {
  return state.phase === 'ready' && !!state.resource && !!state.resource.uri;
}

/** The clipboard payload for the confirm action (the 2nd and final user action). */
export function copyPayload(state) {
  if (!canConfirm(state)) return null;
  const { name, uri } = state.resource;
  // A human label + the machine URI — useful pasted into either prose or a field.
  return name && name !== uri ? `${name}\n${uri}` : uri;
}

/** U3: is this a state the UI must explain (vs. a normal phase)? */
export function explanation(state) {
  return state.phase === 'error' ? state.message : null;
}

/**
 * Are BOTH macOS permissions in place? Automation is needed to read the active
 * app; Accessibility is needed only to script another app's MENUS (apps like Bear
 * with no API). Accessibility is NOT required for the global hotkey — macOS
 * registers those through Carbon, which needs no TCC grant, and a user proved it
 * by pressing ⌘⇧K with Accessibility off.
 *
 * This is therefore "everything granted", NOT the gate for using the app: see
 * canUseCapture(), which keys on Automation alone. Gating the app on this is what
 * locked out a user who had Automation granted. Pure, so it's testable.
 */
export function permissionsComplete(report) {
  return !!report && report.accessibility === true && report.automation === 'granted';
}

/** Every shortcut ubilink registers, in the order the user should hear about
 * failures. Keys match the Rust `HotkeyReport` fields. */
const HOTKEY_LABELS = [
  { key: 'toggle', label: '⌘⇧K (capture)' },
  { key: 'browse', label: '⌘⇧L (browse)' },
  { key: 'settings', label: '⌘⇧, (settings)' },
];

/** "a" / "a and b" / "a, b, and c" — English list joining for naming exactly
 * which shortcuts failed, without three stacked sentences. */
function joinNames(names) {
  if (names.length <= 1) return names[0] ?? '';
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.slice(0, -1).join(', ')}, and ${names[names.length - 1]}`;
}

/**
 * Defect 1/2 (hotkey registration is invisible + never retried): decide what, if
 * anything, to tell the user about the global hotkeys. Pure — `hotkeys` is the
 * Rust `HotkeyReport` ({toggle, browse, settings}: bool, `error`: string|null).
 *
 * Returns '' only when EVERY shortcut registered. `hotkeysRegistered()` (panel.js)
 * blocks onboarding on any single failure, so this must name ALL of them — naming
 * only `toggle` left browse/settings failures stuck on onboarding with no
 * explanation at all (round 2 of this fix). Several failures at once are folded
 * into one coherent sentence rather than one line per shortcut.
 *
 * Round 3 (Defect 5): this used to blame Accessibility whenever it wasn't
 * granted. That was FALSE — macOS global hotkeys register via Carbon
 * `RegisterEventHotKey`, which needs no TCC grant at all (Accessibility is
 * only for AX-based menu-scripting adapters, e.g. Bear). A hotkey that fails
 * to register is always a name conflict with another app, regardless of
 * permission state, so this no longer takes a `PermissionReport` at all.
 */
export function hotkeyWarning(hotkeys) {
  if (!hotkeys) return '';
  const failed = HOTKEY_LABELS.filter((h) => !hotkeys[h.key]);
  if (failed.length === 0) return '';
  const names = joinNames(failed.map((h) => h.label));
  const plural = failed.length > 1;
  return `${names} didn't register — another app is likely already using ${plural ? 'them' : 'it'}. Use the tray icon ("Show ubilink") to open the panel, then free up the shortcut${plural ? 's' : ''} and press Re-check.`;
}

/**
 * Defect 5 (wrong permission gate): capture from browsers/Finder goes through
 * osascript — Automation only. Accessibility is solely for AX-based
 * menu-scripting adapters (e.g. Bear's "Copy Link"). Gating the whole app
 * behind BOTH permissions trapped a user with Automation granted and
 * Accessibility not on the setup screen, unable to use ubilink at all.
 * `permissionsComplete()` (both) still decides whether EVERYTHING, AX-based
 * adapters included, is finished — this decides only whether the capture view
 * is reachable at all.
 */
export function canUseCapture(report) {
  return !!report && report.automation === 'granted';
}

/**
 * Defect 3 (S4 dead end): macOS evaluates Accessibility trust at PROCESS
 * LAUNCH. A process already running when the user grants it in System
 * Settings keeps seeing `AXIsProcessTrusted() === false` until it is quit and
 * reopened — the other cause of the identical symptom is that the entry
 * enabled in Settings belongs to a DIFFERENT Ubilink build/signing identity.
 * Pure decision of which message (if any) to show about Accessibility
 * specifically, on top of the plain ✓/✗ row — tested without a real grant +
 * relaunch cycle.
 *
 * `attemptedGrant`: has the user been through the grant flow this session
 * (clicked "Grant Access…" or "Re-check" at least once)? Before that, a false
 * reading is just the normal starting state — no relaunch talk yet.
 */
export function accessibilityGuidance(report, attemptedGrant) {
  if (!report) return '';
  if (report.accessibility !== true) {
    if (!attemptedGrant) return '';
    return "Still showing as not granted? macOS only checks Accessibility when Ubilink launches, so if you just enabled it in System Settings, quit and reopen Ubilink for it to take effect. (If you enabled a different Ubilink build, this one needs its own entry.) Use Relaunch below.";
  }
  if (report.automation !== 'granted') {
    return 'Accessibility is granted. Automation still needs approval — click Open Settings above, allow Ubilink, then press Re-check.';
  }
  return '';
}
