// Satisfies: U1/U2/U3/U4/U5 — the thin view shell. It owns ONLY DOM + Tauri I/O;
// all decision logic is in the headlessly-tested reduce()/canConfirm()/copyPayload()
// from panel-state.mjs. This file is the ENV-blocked layer (needs a real Tauri
// webview + the Rust command layer), verified via docs/RUNBOOK-gui-panel.md.
import { reduce, initialCapturing, canConfirm, copyPayload, explanation } from './panel-state.mjs';

// Tauri v2 exposes invoke under window.__TAURI__.core; degrade gracefully if absent
// (e.g. opened in a plain browser for layout work).
const invoke = (cmd, args) =>
  window.__TAURI__?.core?.invoke
    ? window.__TAURI__.core.invoke(cmd, args)
    : Promise.reject(new Error('Tauri bridge unavailable'));

const $ = (id) => document.getElementById(id);
let state = initialCapturing; // U1: we are already "capturing" at first paint.

function render() {
  const statusLine = $('status-line');
  const phase = state.phase;
  statusLine.dataset.phase = phase;

  // status text — only while actually capturing; ready shows the resource, error
  // shows the explanation, so the "Capturing…" line must not linger under either.
  statusLine.hidden = phase !== 'capturing';
  if (phase === 'capturing') statusLine.textContent = 'Capturing the active resource…';

  // resource (U5 context)
  const ready = phase === 'ready';
  $('resource-name').hidden = !ready;
  $('resource-uri').hidden = !ready;
  if (ready) {
    $('resource-name').textContent = state.resource.name ?? state.resource.uri;
    $('resource-uri').textContent = state.resource.uri;
  }

  // U2: the copy action is enabled exactly when a link is ready.
  $('copy').disabled = !canConfirm(state);

  // U3: explained error, announced via role=alert.
  const err = explanation(state);
  $('error').hidden = !err;
  $('error').textContent = err ?? '';

  // U5: backlinks list.
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
    a.addEventListener('click', (e) => { e.preventDefault(); invoke('open_link', { uri: b.uri }).catch(reportError); });
    li.appendChild(a);
    list.appendChild(li);
  }

  // Keyboard-first focus management (U4): focus the action as soon as it's usable.
  if (canConfirm(state)) $('copy').focus();
}

function dispatch(event) { state = reduce(state, event); render(); }
function reportError(e) { dispatch({ type: 'failed', message: e?.message ?? String(e) }); }

async function confirmCopy() {
  const payload = copyPayload(state);
  if (!payload) return;
  try {
    await invoke('copy_to_clipboard', { text: payload });
    await invoke('dismiss_panel'); // U2: done in 2 actions; the panel gets out of the way.
  } catch (e) { reportError(e); }
}

// The full open flow: render instantly (U1), then fill from the Rust command.
async function summon() {
  dispatch({ type: 'open' }); // synchronous first paint — does NOT await capture
  try {
    // capture_current returns { resource, backlinks } or throws a user-facing message.
    const view = await invoke('capture_current');
    dispatch({ type: 'captured', resource: view.resource, backlinks: view.backlinks ?? [] });
  } catch (e) { reportError(e); }
}

$('copy').addEventListener('click', confirmCopy);
document.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && canConfirm(state)) { e.preventDefault(); confirmCopy(); }
  if (e.key === 'Escape') invoke('dismiss_panel').catch(() => {});
});

render();   // paint the spinner immediately
summon();   // kick off capture off the critical path
