// Satisfies: U1/U2/U3/U4/U5 + S4 onboarding — the thin view shell. All decision
// logic is the headlessly-tested reduce()/canConfirm()/copyPayload()/
// permissionsComplete() from panel-state.mjs. This file is the ENV-blocked layer
// (needs a real Tauri webview + the Rust command/event layer); verified via
// docs/RUNBOOK-gui-panel.md. Capture is driven by 'capture-ok'/'capture-err' events
// the Rust hotkey handler emits — so it links the app you were IN, every time.
import { reduce, canConfirm, copyPayload, explanation, permissionsComplete } from './panel-state.mjs';
import { initialBrowse, reduce as browseReduce, selectedItem, breadcrumb, atRoot, markedUri } from './browse-state.mjs';

const invoke = (cmd, args) =>
  window.__TAURI__?.core?.invoke
    ? window.__TAURI__.core.invoke(cmd, args)
    : Promise.reject(new Error('Tauri bridge unavailable'));
const onEvent = (name, cb) =>
  window.__TAURI__?.event?.listen ? window.__TAURI__.event.listen(name, cb) : Promise.resolve(() => {});

const $ = (id) => document.getElementById(id);
let state = { phase: 'idle' };
let browse = initialBrowse();
let view = 'capture'; // 'capture' | 'onboarding' | 'browser' — routes keyboard input

// ── section switching ──
function showCaptureView() { $('onboarding').hidden = true; $('browser').hidden = true; $('capture').hidden = false; view = 'capture'; }
function showOnboarding() { $('capture').hidden = true; $('browser').hidden = true; $('onboarding').hidden = false; view = 'onboarding'; }
function showBrowserView() { $('onboarding').hidden = true; $('capture').hidden = true; $('browser').hidden = false; view = 'browser'; }

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

// ── link-browser (Cmd+Shift+L) ──
// Renders the current browse view-model. Titles only (never the raw URI); the
// selection is an aria listbox option so VoiceOver announces it (U4).
function renderBrowser() {
  const trail = breadcrumb(browse);
  $('browser-heading').textContent = atRoot(browse) ? 'Your links' : (trail[trail.length - 1] || 'Links');
  const trailEl = $('browser-trail');
  trailEl.hidden = trail.length === 0;
  trailEl.textContent = trail.length ? `Linked to ${trail.join(' › ')}` : '';
  $('browser-empty').hidden = browse.items.length !== 0;

  const marked = markedUri(browse);
  const statusEl = $('browser-status');
  if (marked) {
    const m = browse.items.find((x) => x.uri === marked);
    statusEl.hidden = false;
    statusEl.textContent = `Linking from “${m ? m.name || m.uri : '…'}” — move to another and press L (Esc cancels)`;
  } else {
    statusEl.hidden = true;
    statusEl.textContent = '';
  }

  const list = $('browser-list');
  list.replaceChildren();
  browse.items.forEach((it, i) => {
    const li = document.createElement('li');
    li.id = `browse-item-${i}`;
    li.setAttribute('role', 'option');
    const selected = i === browse.selected;
    li.setAttribute('aria-selected', selected ? 'true' : 'false');
    if (selected) li.classList.add('sel');
    if (it.uri === marked) li.classList.add('marked');
    li.textContent = it.name || it.uri; // title, not the link
    li.addEventListener('click', () => { browse = browseReduce(browse, { type: 'move', delta: i - browse.selected }); renderBrowser(); });
    li.addEventListener('dblclick', openSelected);
    list.appendChild(li);
  });
  const has = browse.items.length > 0;
  list.setAttribute('aria-activedescendant', has ? `browse-item-${browse.selected}` : '');
  if (has) document.getElementById(`browse-item-${browse.selected}`)?.scrollIntoView({ block: 'nearest' });
}

/** Link the marked resource to `target`, then reload the list with feedback. */
async function linkMarkedTo(target) {
  const a = markedUri(browse);
  if (!a || a === target.uri) return;
  const aName = browse.items.find((x) => x.uri === a)?.name ?? a;
  try {
    await invoke('link_resources', { aUri: a, bUri: target.uri });
    const list = await invoke('list_links');
    browse = browseReduce(initialBrowse(), { type: 'load', items: list ?? [] });
    renderBrowser();
    const s = $('browser-status'); // flash confirmation (renderBrowser cleared it)
    s.hidden = false;
    s.textContent = `Linked “${aName}” ↔ “${target.name || target.uri}” — press → to see it`;
  } catch (e) {
    const s = $('browser-status');
    s.hidden = false;
    s.textContent = `Couldn't link: ${e?.message ?? e}`;
  }
}

function openSelected() {
  const it = selectedItem(browse);
  if (it) invoke('open_link', { uri: it.uri }).catch(reportError);
}
async function drillSelected() {
  const it = selectedItem(browse);
  if (!it) return;
  try {
    const links = await invoke('links_of', { uri: it.uri });
    browse = browseReduce(browse, { type: 'drill', items: links ?? [] });
    renderBrowser();
  } catch { /* stay put; a failed drill leaves the current view intact */ }
}
function browseBack() {
  if (atRoot(browse)) { invoke('dismiss_panel').catch(() => {}); return; } // ← at root dismisses
  browse = browseReduce(browse, { type: 'back' });
  renderBrowser();
}

// ── U1 perf harness (inert unless Rust emits 'perf-mark', i.e. UBILINK_PERF=1) ──
// Measures hotkey-press → first-paint over many invocations and surfaces p50/p99
// in-panel (release bundles have no devtools, so we can't rely on the console).
const __perf = { t0: null, samples: [] };
function __percentiles() {
  const s = [...__perf.samples].sort((a, b) => a - b);
  if (!s.length) return null;
  const q = (p) => s[Math.min(s.length - 1, Math.ceil(p * s.length) - 1)];
  return { n: s.length, p50: q(0.5), p99: q(0.99), min: s[0], max: s[s.length - 1] };
}
// Console accessor for a dev session: window.__ubilinkPerf()
window.__ubilinkPerf = () => { const r = __percentiles(); if (r) console.log(`[ubilink-perf] n=${r.n} p50=${r.p50}ms p99=${r.p99}ms (target p50<80 p99<200) min=${r.min} max=${r.max}`); return r; };
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
  // Link-browser navigation (Cmd+Shift+L view): keyboard-first (U4).
  if (view === 'browser') {
    switch (e.key) {
      case 'ArrowDown': e.preventDefault(); browse = browseReduce(browse, { type: 'move', delta: 1 }); renderBrowser(); return;
      case 'ArrowUp': e.preventDefault(); browse = browseReduce(browse, { type: 'move', delta: -1 }); renderBrowser(); return;
      case 'Enter': e.preventDefault(); openSelected(); return;       // open the link
      case 'ArrowRight': e.preventDefault(); drillSelected(); return; // see what it's linked to
      case 'ArrowLeft': e.preventDefault(); browseBack(); return;     // back out (or dismiss at root)
      case 'l': case 'L': {
        e.preventDefault();
        const cur = selectedItem(browse);
        if (!cur) return;
        const m = markedUri(browse);
        if (m && m !== cur.uri) linkMarkedTo(cur);                    // second L on a different item -> link
        else { browse = browseReduce(browse, { type: 'mark' }); renderBrowser(); } // first L (or toggle off)
        return;
      }
      case 'Escape':
        e.preventDefault();
        if (markedUri(browse)) { browse = browseReduce(browse, { type: 'unmark' }); renderBrowser(); } // cancel link mode first
        else invoke('dismiss_panel').catch(() => {});
        return;
      default: return;
    }
  }
  if (e.key === 'Enter' && canConfirm(state)) { e.preventDefault(); confirmCopy(); }
  if (e.key === 'Escape') invoke('dismiss_panel').catch(() => {});
});

// Cmd+Shift+L: the Rust handler lists the graph and pushes it here.
onEvent('browse', (e) => {
  browse = browseReduce(initialBrowse(), { type: 'load', items: e.payload ?? [] });
  showBrowserView();
  renderBrowser();
  $('browser-list').focus();
});
onEvent('browse-err', (e) => { showCaptureView(); dispatch({ type: 'failed', message: e.payload }); });

// Render-first (U1): the hotkey handler fires 'capturing' BEFORE running capture, so
// the spinner paints immediately. The optional payload is t0 (epoch-ms, UBILINK_PERF
// only) — we measure Date.now()-t0 at this first paint, the real hotkey->visible time.
onEvent('capturing', (e) => {
  if (e.payload != null) __perf.t0 = e.payload;
  showCaptureView();
  dispatch({ type: 'open' }); // -> capturing phase: spinner, no capture data yet
  __markPaint();
});

// Capture results pushed by the Rust hotkey handler (captures the app you were IN).
onEvent('capture-ok', (e) => { showCaptureView(); dispatch({ type: 'captured', resource: e.payload.resource, backlinks: e.payload.backlinks ?? [] }); });
onEvent('capture-err', async (e) => {
  showCaptureView();
  dispatch({ type: 'failed', message: e.payload });
  // If the failure is because a permission is missing, route to setup so the user
  // knows what to fix (rather than just seeing an opaque error).
  const report = await checkPermissions();
  if (report && !permissionsComplete(report)) { renderPermStatuses(report); showOnboarding(); }
});

// Boot: check permissions (shows onboarding or the capture view), then paint.
refreshPermissions({ resetIdle: true });
render();
