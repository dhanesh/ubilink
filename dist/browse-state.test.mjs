// Headless tests for the link-browser view reducer (Cmd+Shift+L). Pure, zero-dep,
// node --test — same discipline as panel-state.test.mjs (keeps S2 honest).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { initialBrowse, reduce, selectedItem, breadcrumb, atRoot, markedUri } from './browse-state.mjs';

const items = (...names) => names.map((n) => ({ name: n, uri: `x://${n}` }));

test('load shows the list at the top, no trail', () => {
  const s = reduce(initialBrowse(), { type: 'load', items: items('A', 'B', 'C') });
  assert.equal(s.items.length, 3);
  assert.equal(s.selected, 0);
  assert.deepEqual(s.trail, []);
  assert.equal(selectedItem(s).name, 'A');
  assert.equal(atRoot(s), true);
});

test('move clamps at both ends and never goes off-list', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('A', 'B', 'C') });
  s = reduce(s, { type: 'move', delta: -1 }); // already at top
  assert.equal(s.selected, 0);
  s = reduce(s, { type: 'move', delta: 1 });
  s = reduce(s, { type: 'move', delta: 1 });
  s = reduce(s, { type: 'move', delta: 1 }); // past the end
  assert.equal(s.selected, 2);
  assert.equal(selectedItem(s).name, 'C');
});

test('move on an empty list is a no-op (no selection to move)', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: [] });
  assert.equal(selectedItem(s), null);
  s = reduce(s, { type: 'move', delta: 1 });
  assert.equal(s.selected, 0);
  assert.equal(selectedItem(s), null);
});

test('drill pushes a breadcrumb frame and shows the linked set at top', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('Note', 'Paper', 'Photo') });
  s = reduce(s, { type: 'move', delta: 1 }); // select "Paper"
  s = reduce(s, { type: 'drill', items: items('Note', 'Sketch') }); // what Paper links to
  assert.equal(selectedItem(s).name, 'Note');
  assert.equal(s.selected, 0);
  assert.equal(atRoot(s), false);
  assert.deepEqual(breadcrumb(s), ['Paper']); // we drilled into Paper
});

test('back restores the exact previous list + selection', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('Note', 'Paper', 'Photo') });
  s = reduce(s, { type: 'move', delta: 2 }); // "Photo"
  s = reduce(s, { type: 'drill', items: items('Paper') });
  s = reduce(s, { type: 'back' });
  assert.equal(selectedItem(s).name, 'Photo'); // selection preserved
  assert.equal(atRoot(s), true);
  assert.deepEqual(breadcrumb(s), []);
});

test('back at the root is a no-op', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('A') });
  s = reduce(s, { type: 'back' });
  assert.equal(atRoot(s), true);
  assert.equal(selectedItem(s).name, 'A');
});

test('drill with no selection (empty list) does nothing', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: [] });
  s = reduce(s, { type: 'drill', items: items('X') });
  assert.equal(atRoot(s), true); // no frame pushed
  assert.equal(selectedItem(s), null);
});

test('drilling into a resource with nothing linked shows an empty list, back still works', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('A', 'B') });
  s = reduce(s, { type: 'drill', items: [] }); // A links to nothing
  assert.equal(s.items.length, 0);
  assert.equal(selectedItem(s), null);
  assert.equal(atRoot(s), false);
  s = reduce(s, { type: 'back' });
  assert.equal(selectedItem(s).name, 'A');
});

test('mark picks up the selected resource for linking; pressing again toggles it off', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('A', 'B') });
  assert.equal(markedUri(s), null);
  s = reduce(s, { type: 'mark' }); // mark A
  assert.equal(markedUri(s), 'x://A');
  s = reduce(s, { type: 'move', delta: 1 }); // move keeps the mark
  assert.equal(markedUri(s), 'x://A');
  assert.equal(selectedItem(s).name, 'B');
  s = reduce(s, { type: 'mark' }); // mark B (different) — now B is the source
  assert.equal(markedUri(s), 'x://B');
  s = reduce(s, { type: 'mark' }); // same again -> toggle off
  assert.equal(markedUri(s), null);
});

test('unmark cancels a pending mark regardless of where the selection is', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('A', 'B') });
  s = reduce(s, { type: 'mark' }); // mark A
  s = reduce(s, { type: 'move', delta: 1 }); // selection now on B, mark still A
  s = reduce(s, { type: 'unmark' });
  assert.equal(markedUri(s), null);
});

test('marking on an empty list does nothing', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: [] });
  s = reduce(s, { type: 'mark' });
  assert.equal(markedUri(s), null);
});

test('navigation (load/drill/back) clears any pending mark', () => {
  let s = reduce(initialBrowse(), { type: 'load', items: items('A', 'B') });
  s = reduce(s, { type: 'mark' });
  s = reduce(s, { type: 'drill', items: items('B') });
  assert.equal(markedUri(s), null, 'drill clears the mark');
  s = reduce(s, { type: 'mark' });
  s = reduce(s, { type: 'back' });
  assert.equal(markedUri(s), null, 'back clears the mark');
  s = reduce(s, { type: 'mark' });
  s = reduce(s, { type: 'load', items: items('A') });
  assert.equal(markedUri(s), null, 'load clears the mark');
});

test('unknown events leave state unchanged', () => {
  const s0 = reduce(initialBrowse(), { type: 'load', items: items('A', 'B') });
  const s1 = reduce(s0, { type: 'wat' });
  assert.deepEqual(s1, s0);
});
