// Satisfies: U1/U2/U3/U4/U5 + S4 onboarding — the thin view shell. All decision
// logic is the headlessly-tested reduce()/canConfirm()/copyPayload()/
// permissionsComplete() from panel-state.mjs. This file is the ENV-blocked layer
// (needs a real Tauri webview + the Rust command/event layer); verified via
// docs/RUNBOOK-gui-panel.md. Capture is driven by 'capture-ok'/'capture-err' events
// the Rust hotkey handler emits — so it links the app you were IN, every time.
import { reduce, canConfirm, copyPayload, explanation, permissionsComplete } from './panel-state.mjs';

const invoke = (cmd, args) =>
  window.__TAURI__?.core?.invoke
    ? window.__TAURI__.core.invoke(cmd, args)
    : Promise.reject(new Error('Tauri bridge unavailable'));
const onEvent = (name, cb) =>
  window.__TAURI__?.event?.listen ? window.__TAURI__.event.listen(name, cb) : Promise.resolve(() => {});

const $ = (id) => document.getElementById(id);
let state = { phase: 'idle' };

// ── section switching ──
function showCaptureView() { $('onboarding').hidden = true; $('capture').hidden = false; }
function showOnboarding() { $('capture').hidden = true; $('onboarding').hidden = false; }

// ── capture view rendering ──
function render() {
  const phase = state.phase;
  const statusLine = $('status-line');
  statusLine.dataset.phase = phase;
  // Status line only for the transient/idle phases; ready shows the resource,
  // error shows the explanation (so "Capturing…" never lingers under either).
  statusLine.hidden = !(phase === 'capturing' || phase === 'idle');
  if (phase === 'capturing') statusLine.textContent = 'Capturing the active resource…';
  if (phase === 'idle') statusLine.textContent = 'Switch to a supported app (a browser, Finder, Bear, Things…), then press ⌘⇧K to capture its link.';

  const ready = phase === 'ready';
  $('resource-name').hidden = !ready;
  $('resource-uri').hidden = !ready;
  if (ready) {
    $('resource-name').textContent = state.resource.name ?? state.resource.uri;
    $('resource-uri').textContent = state.resource.uri;
  }

  $('copy').disabled = !canConfirm(state);

  const err = explanation(state);
  $('error').hidden = !err;
  $('error').textContent = err ?? '';

  const links = ready ? state.backlinks : [];
  $('backlinks').hidden = links.length === 0;
  const list = $('backlinks-list');
  list.replaceChildren();
  for (const b of links) {
    const li = document.createElement('li');
    const a = document.createElement('a');
    a.href = '#';
    a.textContent = b.name || b.uri;
    a.setAttribute('aria-label', `Open ${b.name || b.uri}`);
    a.addEventListener('click', (ev) => { ev.preventDefault(); invoke('open_link', { uri: b.uri }).catch(reportError); });
    li.appendChild(a);
    list.appendChild(li);
  }

  if (canConfirm(state)) $('copy').focus();
}

function dispatch(event) { state = reduce(state, event); render(); }
function reportError(e) { showCaptureView(); dispatch({ type: 'failed', message: e?.message ?? String(e) }); }

// ── U1 perf harness (inert unless Rust emits 'perf-mark', i.e. HOOKY_PERF=1) ──
// Measures hotkey-press → first-paint over many invocations and surfaces p50/p99
// in-panel (release bundles have no devtools, so we can't rely on the console).
const __perf = { t0: null, samples: [] };
function __percentiles() {
  const s = [...__perf.samples].sort((a, b) => a - b);
  if (!s.length) return null;
  const q = (p) => s[Math.min(s.length - 1, Math.ceil(p * s.length) - 1)];
  return { n: s.length, p50: q(0.5), p99: q(0.99), min: s[0], max: s[s.length - 1] };
}
// Console accessor for a dev session: window.__hookyPerf()
window.__hookyPerf = () => { const r = __percentiles(); if (r) console.log(`[hooky-perf] n=${r.n} p50=${r.p50}ms p99=${r.p99}ms (target p50<80 p99<200) min=${r.min} max=${r.max}`); return r; };
function __renderPerf() {
  const r = __percentiles();
  if (!r) return;
  let el = document.getElementById('perf-readout');
  if (!el) {
    el = document.createElement('p');
    el.id = 'perf-readout';
    el.setAttribute('aria-hidden', 'true'); // measurement chrome, not part of the a11y flow
    el.style.cssText = 'position:fixed;left:6px;right:6px;bottom:4px;margin:0;font:11px ui-monospace,SFMono-Regular,monospace;color:#8a8a8a;text-align:center';
    document.body.appendChild(el);
  }
  const pass = r.p50 < 80 && r.p99 < 200 ? '✓' : '…';
  el.textContent = `U1 ${pass} n=${r.n} · p50=${r.p50}ms · p99=${r.p99}ms (target p50<80 p99<200)`;
}
/** Record one hotkey→paint sample: rAF fires after the browser has painted this frame. */
function __markPaint() {
  if (__perf.t0 == null) return;
  const t0 = __perf.t0; __perf.t0 = null;
  requestAnimationFrame(() => { __perf.samples.push(Date.now() - t0); __renderPerf(); });
}

async function confirmCopy() {
  const payload = copyPayload(state);
  if (!payload) return;
  try {
    await invoke('copy_to_clipboard', { text: payload });
    await invoke('dismiss_panel'); // U2: done in 2 actions; get out of the way.
  } catch (e) { reportError(e); }
}

// ── onboarding (S4) ──
function setPerm(name, ok, label) {
  const el = document.querySelector(`.perm[data-perm="${name}"] .perm-status`);
  if (!el) return;
  el.dataset.state = ok ? 'ok' : 'missing';
  el.textContent = ok ? '✓' : '✗';
  el.setAttribute('aria-label', `${name}: ${label}`);
}

/** Query the Rust permission_status command; null if unavailable (browser preview). */
async function checkPermissions() {
  try { return await invoke('permission_status'); }
  catch { return null; }
}

/** Paint the per-permission ✓/✗ rows + the hint. Pure DOM. */
function renderPermStatuses(report) {
  setPerm('accessibility', report.accessibility === true, report.accessibility ? 'granted' : 'not granted');
  const autoOk = report.automation === 'granted';
  setPerm('automation', autoOk, report.automation.replace('_', ' '));
  $('onboarding-hint').textContent = permissionsComplete(report)
    ? 'All set — press ⌘⇧K from any supported app.'
    : 'Grant the items above, then press Re-check.';
}

/** Check + switch view. `resetIdle` shows the idle prompt when complete. */
async function refreshPermissions({ resetIdle = false } = {}) {
  const report = await checkPermissions();
  if (!report) { showCaptureView(); render(); return false; } // browser preview
  renderPermStatuses(report);
  const done = permissionsComplete(report);
  if (done) {
    showCaptureView();
    if (resetIdle && state.phase !== 'ready') dispatch({ type: 'reset' });
  } else {
    showOnboarding();
  }
  return done;
}

/** Always open the setup screen (the "Permissions" affordance), even when complete. */
async function openSetup() {
  const report = await checkPermissions();
  if (report) renderPermStatuses(report);
  showOnboarding();
}

// ── wiring ──
for (const b of document.querySelectorAll('.open-settings')) {
  b.addEventListener('click', () => invoke('open_settings', { pane: b.dataset.pane }).catch(() => {}));
}
$('grant-accessibility').addEventListener('click', async () => {
  await invoke('request_accessibility').catch(() => {}); // shows the macOS dialog
  refreshPermissions(); // reflect the new state
});
$('recheck').addEventListener('click', () => refreshPermissions({ resetIdle: true }));
$('setup-done').addEventListener('click', () => refreshPermissions({ resetIdle: true }));
$('open-setup').addEventListener('click', openSetup);
$('copy').addEventListener('click', confirmCopy);
document.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && canConfirm(state)) { e.preventDefault(); confirmCopy(); }
  if (e.key === 'Escape') invoke('dismiss_panel').catch(() => {});
});

// U1 perf: the hotkey handler stamps t0 just before capture (HOOKY_PERF only).
onEvent('perf-mark', (e) => { __perf.t0 = e.payload; });

// Capture results pushed by the Rust hotkey handler (captures the app you were IN).
onEvent('capture-ok', (e) => { showCaptureView(); dispatch({ type: 'captured', resource: e.payload.resource, backlinks: e.payload.backlinks ?? [] }); __markPaint(); });
onEvent('capture-err', async (e) => {
  showCaptureView();
  dispatch({ type: 'failed', message: e.payload });
  __markPaint();
  // If the failure is because a permission is missing, route to setup so the user
  // knows what to fix (rather than just seeing an opaque error).
  const report = await checkPermissions();
  if (report && !permissionsComplete(report)) { renderPermStatuses(report); showOnboarding(); }
});

// Boot: check permissions (shows onboarding or the capture view), then paint.
refreshPermissions({ resetIdle: true });
render();
