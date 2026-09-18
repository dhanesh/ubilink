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
 * Onboarding gate (S4/RT-2): are BOTH macOS permissions in place so the capture
 * flow can work? Accessibility must be trusted (the global hotkey needs it) and
 * automation must be granted (reading the active app). Pure, so it's testable.
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
 * Rust `HotkeyReport` ({toggle, browse, settings}: bool, `error`: string|null) and
 * `report` is the `PermissionReport`. Onboarding keys off PERMISSIONS, not
 * registration, so this is what tells the user when permissions are fine but a
 * hotkey is still dead (e.g. another app already owns ⌘⇧K).
 *
 * Returns '' only when EVERY shortcut registered. `hotkeysRegistered()` (panel.js)
 * blocks onboarding on any single failure, so this must name ALL of them — naming
 * only `toggle` left browse/settings failures stuck on onboarding with no
 * explanation at all (round 2 of this fix). Several failures at once are folded
 * into one coherent sentence rather than one line per shortcut.
 */
export function hotkeyWarning(hotkeys, report) {
  if (!hotkeys) return '';
  const failed = HOTKEY_LABELS.filter((h) => !hotkeys[h.key]);
  if (failed.length === 0) return '';
  const names = joinNames(failed.map((h) => h.label));
  const plural = failed.length > 1;
  if (!permissionsComplete(report) || report.accessibility !== true) {
    return `The ${names} ${plural ? 'hotkeys need' : 'hotkey needs'} Accessibility permission to work. Grant it above, then ${plural ? "they'll" : "it'll"} start working — no relaunch needed.`;
  }
  return `${names} didn't register — another app is likely already using ${plural ? 'them' : 'it'}. Use the tray icon ("Show ubilink") to open the panel, then free up the shortcut${plural ? 's' : ''} and press Re-check.`;
}
