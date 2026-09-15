// Satisfies: U1/U2/U3/U4/U5 + S4 onboarding — the thin view shell. All decision
// logic is the headlessly-tested reduce()/canConfirm()/copyPayload()/
// permissionsComplete() from panel-state.mjs. This file is the ENV-blocked layer
// (needs a real Tauri webview + the Rust command/event layer); verified via
// docs/RUNBOOK-gui-panel.md. Capture is driven by 'capture-ok'/'capture-err' events
// the Rust hotkey handler emits — so it links the app you were IN, every time.
import { reduce, canConfirm, copyPayload, explanation, permissionsComplete } from './panel-state.mjs';
import { initialBrowse, reduce as browseReduce, selectedItem, breadcrumb, atRoot, markedUri, visibleItems } from './browse-state.mjs';

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

// ── Alfred-style content-sized window ──
// The window is sized to the content; #panel (height:100vh) fills it and the
// browser list scrolls inside. The desired height is computed DETERMINISTICALLY
// (from the row count for the browser, from scrollHeight for the static views) —
// never by measuring the flex list, which depends on the window height and so
// collapses to a feedback loop. The browser is capped at ~10 rows, then scrolls.
const PANEL_W = 640;
const ROW_H = 50;          // approx height of one two-line row (incl. gap)
const BROWSER_ROWS = 10;   // show up to this many before scrolling
let __lastFitH = 0;
function desiredHeight() {
  const padding = 36; // #panel vertical padding
  if (!$('browser').hidden) {
    const n = Math.max(visibleItems(browse).length, 1);
    const listH = Math.min(n, BROWSER_ROWS) * ROW_H + 8;
    const search = 42; // the filter input
    const status = $('browser-status').hidden ? 0 : 24;
    return padding + 34 /*heading*/ + search + 26 /*hint*/ + 34 /*gaps*/ + status + listH;
  }
  // static views (capture / onboarding / settings) aren't scroll containers — measure directly
  const v = !$('capture').hidden ? $('capture') : !$('settings').hidden ? $('settings') : $('onboarding');
  return v.scrollHeight + padding;
}
function fitWindow() {
  const h = Math.max(96, Math.ceil(desiredHeight()));
  if (Math.abs(h - __lastFitH) < 2) return; // skip redundant resizes (e.g. arrow nav)
  __lastFitH = h;
  // Resize via Rust (set_size) — the JS window API didn't take effect under the
  // transparent window. invoke() is the proven channel.
  invoke('resize_panel', { height: h }).catch(() => {});
}

// ── section switching ──
const VIEWS = ['onboarding', 'capture', 'browser', 'settings'];
function showOnly(id) { for (const v of VIEWS) $(v).hidden = v !== id; }
function showCaptureView() { showOnly('capture'); view = 'capture'; }
function showOnboarding() { showOnly('onboarding'); view = 'onboarding'; }
function showBrowserView() { showOnly('browser'); view = 'browser'; }
function showSettingsView() { showOnly('settings'); view = 'settings'; }

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
  // No auto-focus on the Copy button: ⌘C/↵ copy via the global handler, so the
  // button needn't grab focus (which showed a heavy focus ring after the hotkey).
  fitWindow();
}

function dispatch(event) { state = reduce(state, event); render(); }
function reportError(e) { showCaptureView(); dispatch({ type: 'failed', message: e?.message ?? String(e) }); }

// ── link-browser (Cmd+Shift+L) ──
// Renders the current browse view-model. Titles only (never the raw URI); the
// selection is an aria listbox option so VoiceOver announces it (U4).
function renderBrowser() {
  const trail = breadcrumb(browse);
  const heading = $('browser-heading');
  heading.textContent = atRoot(browse) ? 'Your links' : `Linked to ${trail.join(' › ')}`;
  heading.title = heading.textContent; // full text on hover; the view truncates to one line
  $('browser-trail').hidden = true; // folded into the heading (was redundant + wrapped)

  const marked = markedUri(browse);
  const statusEl = $('browser-status');
  if (marked) {
    const m = browse.items.find((x) => x.uri === marked);
    statusEl.hidden = false;
    statusEl.textContent = `Linking from “${m ? m.name || m.uri : '…'}” — move to another and press ⌘L (Esc cancels)`;
  } else {
    statusEl.hidden = true;
    statusEl.textContent = '';
  }

  const vis = visibleItems(browse);
  const list = $('browser-list');
  list.replaceChildren();
  vis.forEach((it, i) => {
    const li = document.createElement('li');
    li.id = `browse-item-${i}`;
    li.setAttribute('role', 'option');
    const selected = i === browse.selected;
    const isMarked = it.uri === marked;
    li.setAttribute('aria-selected', selected ? 'true' : 'false');
    if (selected) li.classList.add('sel');
    if (isMarked) li.classList.add('marked');

    // Aurora row: source-icon tile · title over dim uri · trailing affordance.
    const dot = document.createElement('span');
    dot.className = 'dot';
    const ic = schemeIcon(it.uri);
    dot.style.background = ic.color;
    dot.innerHTML = ic.glyph;

    const main = document.createElement('span');
    main.className = 'row-main';
    const title = document.createElement('span');
    title.className = 'row-title';
    title.textContent = it.name || it.uri; // title leads
    const sub = document.createElement('span');
    sub.className = 'row-sub';
    sub.textContent = it.uri; // the link, dimmed (Alfred's subtitle)
    main.append(title, sub);

    const key = document.createElement('span');
    key.className = 'row-key';
    key.textContent = isMarked ? 'link source' : selected ? '↵' : '';

    // Link-count indicator (extreme right): how many resources this is linked to.
    const count = document.createElement('span');
    count.className = 'row-count';
    const n = it.links | 0;
    if (n > 0) {
      count.textContent = String(n);
      count.setAttribute('aria-label', `${n} link${n === 1 ? '' : 's'}`);
    } else {
      count.classList.add('zero'); // unlinked — faint dash
      count.textContent = '–';
      count.setAttribute('aria-label', 'no links');
    }

    li.append(dot, main, key, count);
    li.addEventListener('click', () => { browse = browseReduce(browse, { type: 'move', delta: i - browse.selected }); renderBrowser(); });
    li.addEventListener('dblclick', openSelected);
    list.appendChild(li);
  });
  const has = vis.length > 0;
  list.setAttribute('aria-activedescendant', has ? `browse-item-${browse.selected}` : '');
  // empty state: distinguish "no captures yet" from "no filter matches"
  const empty = $('browser-empty');
  empty.hidden = has;
  if (!has) empty.textContent = browse.filter ? 'No links match your filter.' : 'No links captured yet — press ⌘⇧K from a supported app to capture one.';
  fitWindow(); // size the window to the list before scrolling the selection into view
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

// Source-icon tiles (Aurora): a coloured rounded square + a white glyph per source,
// keyed by URI scheme — Bear=book, Things=check, web=globe, Finder=folder.
const SRC_GLYPH = {
  book: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 4h11a1 1 0 0 1 1 1v15H6a2 2 0 0 1-2-2V5a1 1 0 0 1 1-1z"/><path d="M17 4l2 1v14l-2-1"/></svg>',
  globe: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3c2.5 2.6 2.5 15.4 0 18M12 3c-2.5 2.6-2.5 15.4 0 18"/></svg>',
  folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h3.4l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="4"/><path d="M8 12.5l2.5 2.5L16 9"/></svg>',
  link: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 14.5l5-5"/><path d="M11.5 6.5l1.3-1.3a3.7 3.7 0 0 1 5.3 5.3L16.8 12"/><path d="M12.5 17.5l-1.3 1.3a3.7 3.7 0 0 1-5.3-5.3L7.2 12"/></svg>',
};
function schemeIcon(uri) {
  const s = String(uri).split(':', 1)[0].toLowerCase();
  const map = {
    bear: { color: 'var(--src-bear)', glyph: SRC_GLYPH.book },
    things: { color: 'var(--src-things)', glyph: SRC_GLYPH.check },
    file: { color: 'var(--src-finder)', glyph: SRC_GLYPH.folder },
    http: { color: 'var(--src-web)', glyph: SRC_GLYPH.globe },
    https: { color: 'var(--src-web)', glyph: SRC_GLYPH.globe },
    obsidian: { color: '#7c5cff', glyph: SRC_GLYPH.book },
    mailto: { color: 'var(--src-web)', glyph: SRC_GLYPH.globe },
  };
  return map[s] || { color: 'var(--accent)', glyph: SRC_GLYPH.link };
}

// ── Settings (⌘⇧,) ──
function renderSettingsStats(stats) {
  const el = $('set-stats');
  if (stats && typeof stats.resources === 'number') {
    el.textContent = `${stats.resources} link${stats.resources === 1 ? '' : 's'} · ${stats.links} connection${stats.links === 1 ? '' : 's'}`;
  } else {
    el.textContent = '—';
  }
}
async function openSettings(stats) {
  showSettingsView();
  // permission pills
  const report = await checkPermissions();
  const setPill = (id, ok) => { const p = $(id); p.dataset.state = ok ? 'ok' : 'missing'; p.textContent = ok ? 'Granted' : 'Not granted'; };
  if (report) { setPill('set-acc', report.accessibility === true); setPill('set-auto', report.automation === 'granted'); }
  // plugin toggles (Settings › Plugins)
  await renderPlugins();
  // stats (use the pushed payload, else fetch)
  renderSettingsStats(stats ?? (await invoke('graph_stats').catch(() => null)));
  await renderVersion();
  $('set-reset-confirm').hidden = true;
  $('set-feedback').hidden = true;
  fitWindow();
}

// Render the adapter manager: one toggle row per app adapter. Built-ins are shown
// but locked on; every other adapter toggles set_plugin_enabled (persisted in Rust).
// Custom adapters (the user's adapters.json) are trusted local config that run with
// the user's permissions — NOT sandboxed plugins (DECISIONS D14) — so say so.
async function renderPlugins() {
  const ul = $('set-plugins');
  if (!ul) return;
  let plugins = [];
  try { plugins = (await invoke('list_plugins')) ?? []; } catch { plugins = []; }
  ul.innerHTML = '';
  for (const p of plugins) {
    const li = document.createElement('li');
    li.className = 'set-plugin';

    const label = document.createElement('div');
    label.className = 'set-text';
    const name = document.createElement('strong');
    name.textContent = p.app;
    const sub = document.createElement('span');
    sub.className = 'set-sub';
    sub.textContent = p.builtin
      ? 'Built-in'
      : p.user_defined
        ? 'Custom adapter · runs with your permissions'
        : (p.scheme ? `${p.scheme}: link` : 'App adapter');
    label.append(name, sub);

    const toggle = document.createElement('input');
    toggle.type = 'checkbox';
    toggle.className = 'set-toggle';
    toggle.checked = p.enabled;
    toggle.disabled = p.builtin; // built-ins are always on
    toggle.setAttribute('aria-label', `Enable ${p.app}`);
    toggle.addEventListener('change', async () => {
      try {
        await invoke('set_plugin_enabled', { app: p.app, enabled: toggle.checked });
      } catch (e) {
        toggle.checked = !toggle.checked; // revert on failure
        const fb = $('set-feedback'); fb.hidden = false;
        fb.textContent = `Couldn't update ${p.app}: ${e?.message ?? e}`;
      }
    });

    li.append(label, toggle);
    ul.appendChild(li);
  }
}

// Settings footer: "Ubilink <version>" so bug reports can name the build. The version
// comes from Tauri (desktop/Cargo.toml); in a plain-browser preview there is none.
async function renderVersion() {
  const el = $('set-version');
  if (!el) return;
  let v = null;
  try { v = await window.__TAURI__?.app?.getVersion?.(); } catch { v = null; }
  el.hidden = !v;
  if (v) el.textContent = `Ubilink ${v}`;
}
async function settingsExport() {
  try {
    const path = await invoke('export_graph');
    const fb = $('set-feedback'); fb.hidden = false; fb.textContent = `Exported to ${path}`;
  } catch (e) { const fb = $('set-feedback'); fb.hidden = false; fb.textContent = `Export failed: ${e?.message ?? e}`; }
}
function settingsImportPick() { $('set-import-file').click(); }
async function settingsImportFile(file) {
  const fb = $('set-feedback'); fb.hidden = false;
  if (!file) return;
  try {
    const json = await file.text();
    const summary = await invoke('import_graph', { json });
    renderSettingsStats(await invoke('graph_stats').catch(() => null));
    fb.textContent = summary; // e.g. "Imported 12 links · 5 connections"
    fitWindow();
  } catch (e) { fb.textContent = `Import failed: ${e?.message ?? e}`; }
}
async function settingsReset() {
  try {
    const removed = await invoke('reset_graph');
    $('set-reset-confirm').hidden = true;
    renderSettingsStats(await invoke('graph_stats').catch(() => null));
    const fb = $('set-feedback'); fb.hidden = false; fb.textContent = `Deleted ${removed} link${removed === 1 ? '' : 's'}.`;
    fitWindow();
  } catch (e) { const fb = $('set-feedback'); fb.hidden = false; fb.textContent = `Reset failed: ${e?.message ?? e}`; }
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
  fitWindow(); // onboarding doesn't go through render()
  return done;
}

/** Always open the setup screen (the "Permissions" affordance), even when complete. */
async function openSetup() {
  const report = await checkPermissions();
  if (report) renderPermStatuses(report);
  showOnboarding();
  fitWindow();
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
$('browser-search').addEventListener('input', (e) => {
  browse = browseReduce(browse, { type: 'filter', text: e.target.value });
  renderBrowser();
});
// Settings actions
$('set-open-perms').addEventListener('click', openSetup);
$('set-export').addEventListener('click', settingsExport);
$('set-import').addEventListener('click', settingsImportPick);
$('set-import-file').addEventListener('change', (e) => { settingsImportFile(e.target.files?.[0]); e.target.value = ''; });
$('set-reset').addEventListener('click', () => { $('set-reset-confirm').hidden = false; $('set-feedback').hidden = true; fitWindow(); });
$('set-reset-no').addEventListener('click', () => { $('set-reset-confirm').hidden = true; fitWindow(); });
$('set-reset-yes').addEventListener('click', settingsReset);
document.addEventListener('keydown', (e) => {
  // Settings: Esc closes (back to capture).
  if (view === 'settings') {
    if (e.key === 'Escape') { e.preventDefault(); if (!$('set-reset-confirm').hidden) { $('set-reset-confirm').hidden = true; fitWindow(); } else invoke('dismiss_panel').catch(() => {}); }
    return;
  }
  // Link-browser navigation (Cmd+Shift+L view): keyboard-first (U4).
  if (view === 'browser') {
    const mod = e.metaKey || e.ctrlKey;
    switch (e.key) {
      case 'ArrowDown': e.preventDefault(); browse = browseReduce(browse, { type: 'move', delta: 1 }); renderBrowser(); return;
      case 'ArrowUp': e.preventDefault(); browse = browseReduce(browse, { type: 'move', delta: -1 }); renderBrowser(); return;
      case 'Enter': e.preventDefault(); openSelected(); return;       // open the link
      case 'ArrowRight': e.preventDefault(); drillSelected(); return; // see what it's linked to
      case 'ArrowLeft':
        if (browse.filter) return;        // editing the filter — let ← move the cursor
        e.preventDefault(); browseBack(); return;
      case 'l': case 'L': {
        if (!mod) return;                 // plain 'l' types into the filter; ⌘L links
        e.preventDefault();
        const cur = selectedItem(browse);
        if (!cur) return;
        const m = markedUri(browse);
        if (m && m !== cur.uri) linkMarkedTo(cur);
        else { browse = browseReduce(browse, { type: 'mark' }); renderBrowser(); }
        return;
      }
      case 'Escape':
        e.preventDefault();
        if (markedUri(browse)) { browse = browseReduce(browse, { type: 'unmark' }); renderBrowser(); }       // cancel link mode
        else if (browse.filter) { $('browser-search').value = ''; browse = browseReduce(browse, { type: 'filter', text: '' }); renderBrowser(); } // clear filter
        else invoke('dismiss_panel').catch(() => {});
        return;
      default: return; // printable keys fall through to the focused filter input
    }
  }
  if ((e.metaKey || e.ctrlKey) && (e.key === 'c' || e.key === 'C') && canConfirm(state)) { e.preventDefault(); confirmCopy(); } // ⌘C copies
  if (e.key === 'Enter' && canConfirm(state)) { e.preventDefault(); confirmCopy(); }
  if (e.key === 'Escape') invoke('dismiss_panel').catch(() => {});
});

// Cmd+Shift+L: the Rust handler lists the graph and pushes it here.
onEvent('browse', (e) => {
  browse = browseReduce(initialBrowse(), { type: 'load', items: e.payload ?? [] });
  showBrowserView();
  $('browser-search').value = '';
  renderBrowser();
  $('browser-search').focus(); // type-to-filter; arrows/enter still navigate
});
onEvent('browse-err', (e) => { showCaptureView(); dispatch({ type: 'failed', message: e.payload }); });

// Cmd+Shift+, : open Settings (payload = current graph stats, or null).
onEvent('settings', (e) => { openSettings(e.payload); });

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
