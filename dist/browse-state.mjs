// The link-browser view-state: a pure, zero-dep reducer (same discipline as
// panel-state.mjs) so the whole browse/drill/filter/keyboard model is testable
// headlessly. browse.js (in panel.js) is the thin DOM/Tauri shell over this.
//
// Surfaces the captured link graph (Cmd+Shift+L): a list of resources shown by
// TITLE, keyboard navigable, with Enter to open, → to drill into "everything
// linked to this", and a live FILTER (type to narrow). A breadcrumb trail makes
// the drill reversible (back).
//
// State: { items:[{name,uri}], selected:int, trail:[frame], marked:uri|null, filter:str }
//   selected indexes the VISIBLE (filtered) items, not the raw items.
//   frame = { items, selected(full-index), title } — saved view to restore on `back`.
// Events: load · move · drill · back · mark · unmark · filter

export const initialBrowse = () => ({ items: [], selected: 0, trail: [], marked: null, filter: '' });

const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));

/** Items matching the current filter (case-insensitive over title + uri). */
export function visibleItems(state) {
  const f = state.filter.trim().toLowerCase();
  if (!f) return state.items;
  return state.items.filter((it) => `${it.name} ${it.uri}`.toLowerCase().includes(f));
}

export function reduce(state, event) {
  switch (event.type) {
    case 'load':
      return { items: Array.isArray(event.items) ? event.items : [], selected: 0, trail: [], marked: null, filter: '' };

    case 'move': {
      const v = visibleItems(state);
      if (v.length === 0) return state;
      const selected = clamp(state.selected + (event.delta | 0), 0, v.length - 1);
      return selected === state.selected ? state : { ...state, selected };
    }

    case 'filter': {
      const filter = String(event.text ?? '');
      return filter === state.filter ? state : { ...state, filter, selected: 0 };
    }

    case 'drill': {
      const current = selectedItem(state); // the visible-selected resource
      if (!current) return state;
      // store the FULL-items index so `back` restores correctly (filter is cleared)
      const fullIdx = state.items.findIndex((it) => it.uri === current.uri);
      const frame = { items: state.items, selected: fullIdx >= 0 ? fullIdx : 0, title: current.name };
      return {
        items: Array.isArray(event.items) ? event.items : [],
        selected: 0,
        trail: [...state.trail, frame],
        marked: null,
        filter: '',
      };
    }

    case 'back': {
      if (state.trail.length === 0) return state;
      const prev = state.trail[state.trail.length - 1];
      return { items: prev.items, selected: prev.selected, trail: state.trail.slice(0, -1), marked: null, filter: '' };
    }

    case 'mark': {
      const cur = selectedItem(state);
      if (!cur) return state;
      return { ...state, marked: state.marked === cur.uri ? null : cur.uri };
    }

    case 'unmark':
      return state.marked == null ? state : { ...state, marked: null };

    default:
      return state;
  }
}

/** The currently highlighted (visible) resource, or null. */
export function selectedItem(state) {
  const v = visibleItems(state);
  return v.length ? v[state.selected] : null;
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
