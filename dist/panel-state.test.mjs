// Headless tests for the panel view-state (U1/U2/U3/U5). Zero npm deps — uses
// node's built-in runner: `node --test dist/`. Keeps S2 honest (no third-party JS).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { reduce, canConfirm, copyPayload, explanation } from './panel-state.mjs';

test('U1: open() resolves to capturing synchronously, without capture data', () => {
  const s = reduce({ phase: 'idle' }, { type: 'open' });
  assert.equal(s.phase, 'capturing');
  // The first paint carries NO resource — proving render does not block on capture.
  assert.equal(s.resource, undefined);
});

test('U5: a captured resource becomes ready and surfaces its backlinks', () => {
  const s = reduce(
    { phase: 'capturing' },
    { type: 'captured', resource: { name: 'Note', uri: 'file:///n.md' }, backlinks: [{ uri: 'file:///p.pdf' }] },
  );
  assert.equal(s.phase, 'ready');
  assert.equal(s.backlinks.length, 1);
  assert.equal(s.backlinks[0].uri, 'file:///p.pdf');
});

test('U2: link obtained in exactly two actions (open -> confirm)', () => {
  let s = { phase: 'idle' };
  s = reduce(s, { type: 'open' }); // action 1: the hotkey
  assert.equal(canConfirm(s), false, 'cannot confirm before capture lands');
  s = reduce(s, { type: 'captured', resource: { name: 'Doc', uri: 'https://x/y' }, backlinks: [] });
  assert.equal(canConfirm(s), true); // action 2: confirm is now available
  assert.equal(copyPayload(s), 'Doc\nhttps://x/y');
});

test('U2: payload is the bare URI when name equals the URI', () => {
  const s = reduce({ phase: 'capturing' }, { type: 'captured', resource: { name: 'https://x', uri: 'https://x' }, backlinks: [] });
  assert.equal(copyPayload(s), 'https://x');
});

test('U3: a failure always carries an explanation, never silent', () => {
  const s = reduce({ phase: 'capturing' }, { type: 'failed', message: 'Permission needed' });
  assert.equal(s.phase, 'error');
  assert.equal(explanation(s), 'Permission needed');
});

test('U3: an empty failure message is coerced to a real explanation', () => {
  const s = reduce({ phase: 'capturing' }, { type: 'failed', message: '   ' });
  assert.equal(s.phase, 'error');
  assert.ok(explanation(s).length > 0);
});

test('confirm is unavailable in capturing/error/idle states', () => {
  for (const phase of ['idle', 'capturing']) assert.equal(canConfirm({ phase }), false);
  assert.equal(canConfirm({ phase: 'error', message: 'x' }), false);
  assert.equal(copyPayload({ phase: 'capturing' }), null);
});

test('unknown events leave state unchanged', () => {
  const s = { phase: 'ready', resource: { uri: 'u' }, backlinks: [] };
  assert.deepEqual(reduce(s, { type: 'nonsense' }), s);
});
