// Headless tests for the panel view-state (U1/U2/U3/U5). Zero npm deps — uses
// node's built-in runner: `node --test dist/`. Keeps S2 honest (no third-party JS).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { reduce, canConfirm, copyPayload, explanation, permissionsComplete, hotkeyWarning } from './panel-state.mjs';

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

test('S4: onboarding is complete only when BOTH permissions are in place', () => {
  assert.equal(permissionsComplete({ accessibility: true, automation: 'granted' }), true);
  assert.equal(permissionsComplete({ accessibility: false, automation: 'granted' }), false);
  assert.equal(permissionsComplete({ accessibility: true, automation: 'denied' }), false);
  assert.equal(permissionsComplete({ accessibility: true, automation: 'not_determined' }), false);
  assert.equal(permissionsComplete(null), false);
});

test('H1: no warning when every hotkey registered', () => {
  const hotkeys = { toggle: true, browse: true, settings: true, error: null };
  const report = { accessibility: true, automation: 'granted' };
  assert.equal(hotkeyWarning(hotkeys, report), '');
});

test('H2: toggle failed and Accessibility is not granted -> says the hotkey needs it', () => {
  const hotkeys = { toggle: false, browse: true, settings: true, error: 'not permitted' };
  const report = { accessibility: false, automation: 'granted' };
  const msg = hotkeyWarning(hotkeys, report);
  assert.ok(msg.length > 0);
  assert.match(msg, /accessibility/i);
});

test('H3: toggle failed but Accessibility IS granted -> conflict case names the tray', () => {
  const hotkeys = { toggle: false, browse: true, settings: true, error: 'already registered' };
  const report = { accessibility: true, automation: 'granted' };
  const msg = hotkeyWarning(hotkeys, report);
  assert.ok(msg.length > 0);
  // Must not blame a missing permission the user already granted.
  assert.doesNotMatch(msg, /needs? accessibility/i);
  assert.match(msg, /another app/i);
  assert.match(msg, /tray/i);
});

// Round 2: hotkeyWarning used to only look at `toggle`. hotkeysRegistered()
// (panel.js) blocks onboarding on ANY of the three failing, so browse/settings
// failing alone left the user stuck on onboarding with an EMPTY warning — the
// exact bug this feature exists to remove, just moved to a different shortcut.

test('H4: browse (⌘⇧L) fails alone, Accessibility granted -> names ⌘⇧L, not ⌘⇧K', () => {
  const hotkeys = { toggle: true, browse: false, settings: true, error: 'already registered' };
  const report = { accessibility: true, automation: 'granted' };
  const msg = hotkeyWarning(hotkeys, report);
  assert.ok(msg.length > 0);
  assert.match(msg, /⌘⇧L/);
  assert.doesNotMatch(msg, /⌘⇧K/);
  assert.match(msg, /another app/i);
  assert.match(msg, /tray/i);
});

test('H5: settings (⌘⇧,) fails alone, Accessibility not granted -> names ⌘⇧, and needs Accessibility', () => {
  const hotkeys = { toggle: true, browse: true, settings: false, error: 'not permitted' };
  const report = { accessibility: false, automation: 'granted' };
  const msg = hotkeyWarning(hotkeys, report);
  assert.ok(msg.length > 0);
  assert.match(msg, /⌘⇧,/);
  assert.doesNotMatch(msg, /⌘⇧K/);
  assert.doesNotMatch(msg, /⌘⇧L/);
  assert.match(msg, /accessibility/i);
});

test('H6: toggle plus one other (browse) both fail -> both are named, one coherent message', () => {
  const hotkeys = { toggle: false, browse: false, settings: true, error: 'already registered' };
  const report = { accessibility: true, automation: 'granted' };
  const msg = hotkeyWarning(hotkeys, report);
  assert.ok(msg.length > 0);
  assert.match(msg, /⌘⇧K/);
  assert.match(msg, /⌘⇧L/);
  assert.doesNotMatch(msg, /⌘⇧,/);
  // One sentence naming both, not three stacked warnings.
  assert.equal((msg.match(/another app/gi) || []).length, 1);
});

test('H7: all three registered -> no warning, even if permissions are incomplete', () => {
  const hotkeys = { toggle: true, browse: true, settings: true, error: null };
  const report = { accessibility: false, automation: 'granted' };
  assert.equal(hotkeyWarning(hotkeys, report), '');
});
