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
 * Can the user capture? Every built-in adapter goes through osascript, which
 * needs Automation and nothing else. Accessibility is not required: the global
 * hotkey registers via Carbon (no TCC grant), and Bear — the one adapter that
 * used to script menus — now captures through bearcli. Gating the app on
 * Accessibility as well once trapped a user with Automation granted on the
 * setup screen, unable to use Ubilink at all.
 */
export function canUseCapture(report) {
  return !!report && report.automation === 'granted';
}

/** Views where losing focus means the user has moved on, so the panel gets out of
 * the way (like Spotlight). Onboarding and settings are deliberately absent:
 * leaving the panel IS the task there (granting a permission in System Settings,
 * picking a file to import), so dismissing would pull the rug out mid-step. */
const DISMISS_ON_BLUR_VIEWS = new Set(['capture', 'browser']);

/**
 * Should the panel hide when it loses focus? Pure, so it's testable.
 * Never mid-capture: the first capture from an app can raise a macOS Automation
 * prompt that takes focus, and hiding then would drop the result on the floor.
 * An unknown view keeps the panel open — failing safe beats vanishing.
 */
export function shouldDismissOnBlur(view, phase) {
  return DISMISS_ON_BLUR_VIEWS.has(view) && phase !== 'capturing';
}
