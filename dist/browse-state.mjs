// The link-browser view-state: a pure, zero-dep reducer (same discipline as
// panel-state.mjs) so the whole browse/drill/keyboard model is testable headlessly.
// browse.js (in panel.js) is the thin DOM/Tauri shell over this.
//
// Surfaces the captured link graph (Cmd+Shift+L): a list of resources shown by
// TITLE (never the raw URI), keyboard navigable (move), with Enter to open and
// → to drill into "everything linked to this resource". A breadcrumb trail makes
// the drill reversible (back).
//
// State shape: { items: [{name, uri}], selected: int, trail: [frame], marked: uri|null }
//   frame = { items, selected, title } — a saved view to restore on `back`.
//   marked = the uri "picked up" for link-creation (L), or null. Reset on any nav.
// Events:
//   {type:'load',  items}  -> fresh list at the top, trail+mark cleared (the L hotkey)
//   {type:'move',  delta}  -> move the selection, clamped to the list
//   {type:'drill', items}  -> push the current view, show `items` (links of selection)
//   {type:'back'}          -> pop the trail, restoring the prior view exactly
//   {type:'mark'}          -> toggle the selected resource as the link source (L)

export const initialBrowse = () => ({ items: [], selected: 0, trail: [], marked: null });

const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));

export function reduce(state, event) {
  switch (event.type) {
    case 'load':
      return { items: Array.isArray(event.items) ? event.items : [], selected: 0, trail: [], marked: null };

    case 'move': {
      if (state.items.length === 0) return state;
      const selected = clamp(state.selected + (event.delta | 0), 0, state.items.length - 1);
      return selected === state.selected ? state : { ...state, selected };
    }

    case 'drill': {
      const current = selectedItem(state);
      if (!current) return state; // nothing selected -> nothing to drill into
      const frame = { items: state.items, selected: state.selected, title: current.name };
      return {
        items: Array.isArray(event.items) ? event.items : [],
        selected: 0,
        trail: [...state.trail, frame],
        marked: null, // marking is root-scoped; drilling clears it
      };
    }

    case 'back': {
      if (state.trail.length === 0) return state;
      const prev = state.trail[state.trail.length - 1];
      return { items: prev.items, selected: prev.selected, trail: state.trail.slice(0, -1), marked: null };
    }

    case 'mark': {
      const cur = selectedItem(state);
      if (!cur) return state; // nothing to pick up
      return { ...state, marked: state.marked === cur.uri ? null : cur.uri }; // toggle
    }

    case 'unmark':
      return state.marked == null ? state : { ...state, marked: null };

    default:
      return state;
  }
}

/** The currently highlighted resource, or null when the list is empty. */
export function selectedItem(state) {
  return state.items.length ? state.items[state.selected] : null;
}

/** Titles of the resources we've drilled through, outermost first. */
export function breadcrumb(state) {
  return state.trail.map((f) => f.title);
}

/** True at the top level (no drill) — the back key should then dismiss instead. */
export function atRoot(state) {
  return state.trail.length === 0;
}

/** The uri picked up for linking (L), or null. */
export function markedUri(state) {
  return state.marked ?? null;
}
