// Headless tests for the panel view-state (U1/U2/U3/U5). Zero npm deps — uses
// node's built-in runner: `node --test dist/`. Keeps S2 honest (no third-party JS).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  reduce,
  canConfirm,
  copyPayload,
  explanation,
  permissionsComplete,
  hotkeyWarning,
  accessibilityGuidance,
  canUseCapture,
} from './panel-state.mjs';

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
  assert.equal(hotkeyWarning(hotkeys), '');
});

// Round 3 (Defect 5): hotkeyWarning used to blame Accessibility whenever it
// wasn't granted. That was FALSE — macOS global hotkeys register via Carbon
// RegisterEventHotKey, which needs no TCC grant at all (unlike AX-based
// menu-scripting adapters, e.g. Bear). A hotkey that fails to register is a
// NAME CONFLICT with another app, full stop, regardless of permission state —
// the message must say that and must never send anyone chasing Accessibility
// for a problem it cannot fix.

test('H2: toggle fails to register while Accessibility also happens to be ungranted -> still a conflict, never blames Accessibility', () => {
  const hotkeys = { toggle: false, browse: true, settings: true, error: 'not permitted' };
  const msg = hotkeyWarning(hotkeys);
  assert.ok(msg.length > 0);
  assert.doesNotMatch(msg, /accessibility/i);
  assert.match(msg, /another app/i);
  assert.match(msg, /tray/i);
});

test('H3: toggle failed (Accessibility granted too) -> conflict case names the tray', () => {
  const hotkeys = { toggle: false, browse: true, settings: true, error: 'already registered' };
  const msg = hotkeyWarning(hotkeys);
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

test('H4: browse (⌘⇧L) fails alone -> names ⌘⇧L, not ⌘⇧K', () => {
  const hotkeys = { toggle: true, browse: false, settings: true, error: 'already registered' };
  const msg = hotkeyWarning(hotkeys);
  assert.ok(msg.length > 0);
  assert.match(msg, /⌘⇧L/);
  assert.doesNotMatch(msg, /⌘⇧K/);
  assert.match(msg, /another app/i);
  assert.match(msg, /tray/i);
});

test('H5: settings (⌘⇧,) fails alone, even with Accessibility ungranted -> names ⌘⇧, as a conflict, not an Accessibility need', () => {
  const hotkeys = { toggle: true, browse: true, settings: false, error: 'not permitted' };
  const msg = hotkeyWarning(hotkeys);
  assert.ok(msg.length > 0);
  assert.match(msg, /⌘⇧,/);
  assert.doesNotMatch(msg, /⌘⇧K/);
  assert.doesNotMatch(msg, /⌘⇧L/);
  assert.doesNotMatch(msg, /accessibility/i);
  assert.match(msg, /another app/i);
});

test('H6: toggle plus one other (browse) both fail -> both are named, one coherent message', () => {
  const hotkeys = { toggle: false, browse: false, settings: true, error: 'already registered' };
  const msg = hotkeyWarning(hotkeys);
  assert.ok(msg.length > 0);
  assert.match(msg, /⌘⇧K/);
  assert.match(msg, /⌘⇧L/);
  assert.doesNotMatch(msg, /⌘⇧,/);
  // One sentence naming both, not three stacked warnings.
  assert.equal((msg.match(/another app/gi) || []).length, 1);
});

test('H7: all three registered -> no warning, even if permissions are incomplete', () => {
  const hotkeys = { toggle: true, browse: true, settings: true, error: null };
  assert.equal(hotkeyWarning(hotkeys), '');
});

// Defect 3 (S4 dead end): Accessibility trust is evaluated by macOS at PROCESS
// LAUNCH. A process already running when the user grants it in System
// Settings keeps seeing itself as untrusted until quit and reopened — so once
// the user has been through the grant flow, a still-false reading needs to
// say that plainly and offer a way out, instead of repeating "grant it above"
// forever.

test('A1: Accessibility still false after the user has been through the grant flow -> explains launch-time trust and offers relaunch', () => {
  const msg = accessibilityGuidance({ accessibility: false, automation: 'granted' }, true);
  assert.ok(msg.length > 0);
  assert.match(msg, /launch/i);
  assert.match(msg, /reopen|relaunch|quit/i);
  assert.match(msg, /different/i); // names the other-build cause too
});

test('A2: Accessibility granted but Automation still missing -> asks for Automation, not a relaunch', () => {
  const msg = accessibilityGuidance({ accessibility: true, automation: 'not_determined' }, false);
  assert.ok(msg.length > 0);
  assert.match(msg, /automation/i);
  assert.doesNotMatch(msg, /launch/i);
});

test('A3: everything granted -> no special guidance needed', () => {
  assert.equal(accessibilityGuidance({ accessibility: true, automation: 'granted' }, true), '');
});

test('A0: Accessibility false but the user has not attempted a grant yet -> no premature relaunch talk', () => {
  assert.equal(accessibilityGuidance({ accessibility: false, automation: 'granted' }, false), '');
});

// Defect 5 (wrong permission gate): capture from browsers/Finder goes through
// osascript (Automation) only. Accessibility is solely for AX-based
// menu-scripting adapters (e.g. Bear's "Copy Link") — gating the WHOLE app on
// both permissions trapped a user with Automation granted and Accessibility
// not on the setup screen, unable to use ubilink at all.

test('S5: capture is reachable with Automation granted alone — Accessibility is not required to leave onboarding', () => {
  assert.equal(canUseCapture({ accessibility: false, automation: 'granted' }), true);
});

test('S6: capture is NOT reachable without Automation, regardless of Accessibility', () => {
  assert.equal(canUseCapture({ accessibility: true, automation: 'denied' }), false);
  assert.equal(canUseCapture({ accessibility: true, automation: 'not_determined' }), false);
  assert.equal(canUseCapture(null), false);
});

// Dismiss on click-away. Only where leaving the panel means the user is done with
// it; never where leaving IS the task (granting a permission in System Settings,
// picking an import file), and never while a capture is in flight, because the
// first capture from an app can raise a macOS Automation prompt that takes focus.
import { shouldDismissOnBlur } from './panel-state.mjs';

test('D1: clicking away dismisses the capture view once a capture has settled', () => {
  for (const phase of ['idle', 'ready', 'error']) {
    assert.equal(shouldDismissOnBlur('capture', phase), true, phase);
  }
});

test('D2: clicking away dismisses the link browser', () => {
  assert.equal(shouldDismissOnBlur('browser', 'idle'), true);
});

test('D3: never dismisses mid-capture (an Automation prompt may have taken focus)', () => {
  assert.equal(shouldDismissOnBlur('capture', 'capturing'), false);
});

test('D4: never dismisses onboarding or settings, where leaving the panel is the task', () => {
  assert.equal(shouldDismissOnBlur('onboarding', 'idle'), false);
  assert.equal(shouldDismissOnBlur('settings', 'idle'), false);
});

test('D5: an unknown view keeps the panel open (fail safe)', () => {
  assert.equal(shouldDismissOnBlur(undefined, 'idle'), false);
  assert.equal(shouldDismissOnBlur('something-new', 'ready'), false);
});
