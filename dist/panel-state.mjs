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
            : "hooky couldn't capture a link, and isn't sure why. Try again.",
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
