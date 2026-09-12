// Satisfies: RT-5 (binding), S1 (invariant), T1 (boundary), TN1 (capability
// broker), TN6 (isolation proof / manifold evidence E9), S5 (DoS bounds).
// See .manifold/linking-manifesto-app.md and DECISIONS.md D1/D3.
//
// The Node sandbox host. Runs under `node --permission` with NO fs/child/worker
// allowances (and, on macOS, inside a sandbox-exec jail), with an EMPTY
// environment, so this process — and any plugin code it loads — is denied
// ambient filesystem, child_process, worker_threads and network access.
//
// It does exactly three things:
//   1. Receives the plugin SOURCE from the trusted Rust core over stdin and
//      loads it via a `data:` URL import (touches no filesystem).
//   2. Calls the plugin's exported `run({ resource, probe, caps })`.
//   3. Exposes ONLY `caps.request(capability)` to the plugin; every call is
//      forwarded to the Rust broker, which checks it against the manifest.
//
// FRAME INTEGRITY (DECISIONS D3). The plugin shares this realm, so it can write
// to fd 1 and monkeypatch any global. Therefore every host->core frame is
// prefixed with a per-session NONCE the core generated and sent as the very
// first stdin line; the core drops any line without it. The nonce is:
//   - read synchronously with a captured `fs.readSync(0)` BEFORE any plugin
//     code exists in this realm, and never echoed by the core again;
//   - held only in this module's scope (not in argv, env, or any object);
//   - only ever combined with other data by primitive string concatenation and
//     written with a captured `fs.writeSync(1, string)` (the string goes straight
//     to C++; no patchable prototype method sees it);
//   - kept out of heap-introspection reach: v8 heap-snapshot / queryObjects /
//     setFlagsFromString and trace_events are disabled below before the plugin
//     loads (inspector and process.binding are already denied by --permission).
// Frame objects are null-prototype and serialized with a captured JSON.stringify,
// so global JSON / Object.prototype pollution cannot rewrite them.
//
// Residual (documented in DECISIONS D3): the plugin can still READ stdin after
// the nonce line, i.e. the broker's replies to ITS OWN requests (acceptable: one
// host process serves one plugin), and can subvert host logic it shares a realm
// with. The core therefore treats every authenticated frame's payload as
// plugin-controlled and derives every security decision from its own broker.
//
// Protocol: see src-tauri/src/sandbox.rs.

import fs from 'node:fs';
import v8 from 'node:v8';
import traceEvents from 'node:trace_events';
import { syncBuiltinESMExports } from 'node:module';
import { createInterface } from 'node:readline';

// ---- 1. capture primitives while the realm is still pristine ----------------
const readSync = fs.readSync;
const writeSync = fs.writeSync;
const stringify = JSON.stringify;
const parse = JSON.parse;
const apply = Reflect.apply;
const charCodeAt = String.prototype.charCodeAt;
const strSlice = String.prototype.slice;
const encodeURIComponent_ = encodeURIComponent;
const PromiseCtor = Promise;
const exit = process.exit;
const HEX = '0123456789abcdef';
const NONCE_LEN = 64;

const stderrWrite = (s) => {
  try {
    writeSync(2, s);
  } catch {
    /* stderr full or closed: drop, never block the host */
  }
};
const die = (why) => {
  stderrWrite('[plugin-host] fatal: ' + why + '\n');
  apply(exit, process, [2]);
};

// ---- 2. close heap/trace introspection before any plugin code runs ----------
function disabled(name) {
  return function () {
    const e = new Error(name + ' is disabled inside the plugin sandbox');
    e.code = 'ERR_ACCESS_DENIED';
    throw e;
  };
}
for (const k of [
  'getHeapSnapshot', // would expose every string in the heap, incl. the nonce
  'writeHeapSnapshot',
  'setHeapSnapshotNearHeapLimit',
  'queryObjects',
  'setFlagsFromString', // e.g. --allow-natives-syntax
]) {
  if (typeof v8[k] === 'function') v8[k] = disabled('v8.' + k);
}
// trace_events writes node_trace.N.log natively, bypassing --permission.
traceEvents.createTracing = disabled('trace_events.createTracing');
try {
  process.report.writeReport = disabled('process.report.writeReport');
} catch {
  /* not writable on this Node: the OS jail still denies file writes */
}
syncBuiltinESMExports(); // propagate to `import ... from 'node:v8'` namespaces

// ---- 3. read the frame nonce: first stdin line, before the plugin exists ----
const NONCE = (() => {
  const buf = Buffer.alloc(NONCE_LEN + 1);
  let off = 0;
  while (off < buf.length) {
    let n;
    try {
      n = readSync(0, buf, off, buf.length - off, null);
    } catch (e) {
      if (e && e.code === 'EAGAIN') continue;
      die('cannot read nonce: ' + (e && e.code));
    }
    if (n === 0) die('stdin closed before nonce');
    off += n;
  }
  if (buf[NONCE_LEN] !== 10) die('malformed nonce line');
  let s = '';
  for (let i = 0; i < NONCE_LEN; i++) {
    const c = buf[i];
    if (c >= 48 && c <= 57) s += HEX[c - 48];
    else if (c >= 97 && c <= 102) s += HEX[c - 87];
    else die('malformed nonce');
  }
  buf.fill(0);
  return s;
})();

// ---- 4. authenticated, ASCII-only frame writer -------------------------------
// Escape non-ASCII so string offsets == byte offsets (lets us resume a partial
// write on a non-blocking fd without re-encoding). JSON.stringify already
// escapes control characters, so a frame is always exactly one line.
function toAscii(s) {
  let out = '';
  let start = 0;
  const n = s.length;
  for (let i = 0; i < n; i++) {
    const c = apply(charCodeAt, s, [i]);
    if (c > 0x7e) {
      out +=
        apply(strSlice, s, [start, i]) +
        '\\u' + HEX[(c >> 12) & 15] + HEX[(c >> 8) & 15] + HEX[(c >> 4) & 15] + HEX[c & 15];
      start = i + 1;
    }
  }
  return start === 0 ? s : out + apply(strSlice, s, [start]);
}

function writeAll(line) {
  let off = 0;
  const n = line.length;
  while (off < n) {
    let w;
    try {
      w = writeSync(1, off === 0 ? line : apply(strSlice, line, [off]));
    } catch (e) {
      // A plugin that touches fd 1 as a stream can make it non-blocking.
      if (e && e.code === 'EAGAIN') continue;
      apply(exit, process, [3]); // EPIPE etc: the core is gone
    }
    off += w;
  }
}

// `frame` MUST be a null-prototype object. Returns false if it is not
// JSON-serializable (e.g. a plugin result containing a BigInt or a cycle).
function send(frame) {
  let json;
  try {
    json = stringify(frame);
  } catch {
    json = undefined;
  }
  if (typeof json !== 'string') return false;
  writeAll(NONCE + ' ' + toAscii(json) + '\n');
  return true;
}

// ---- 5. keep stdout protocol-only (hygiene, not the security boundary) -------
const pluginConsole = (...a) => stderrWrite('[plugin-console] ' + a.map(String).join(' ') + '\n');
console.log = pluginConsole;
console.info = pluginConsole;
console.warn = pluginConsole;
console.error = pluginConsole;
console.debug = pluginConsole;
// `process.stdout` now resolves to stderr: casual plugin writes are visible in
// logs, and fd 1 is never wrapped in a (non-blocking) stream by the host. Any
// write that still reaches fd 1 lacks the nonce and is dropped by the core.
Object.defineProperty(process, 'stdout', {
  configurable: true,
  enumerable: true,
  get: () => process.stderr,
});

// ---- capability request plumbing -------------------------------------------
let nextId = 1;
const pending = { __proto__: null }; // id -> resolve

function requestCapability(cap) {
  const id = nextId++;
  return new PromiseCtor((resolve) => {
    pending[id] = resolve;
    send({ __proto__: null, t: 'cap_request', id, cap });
  });
}

// The ONLY API surface handed to untrusted plugin code.
const caps = {
  // Returns { allowed, result, reason }. The broker is the sole arbiter.
  async request(cap) {
    if (typeof cap !== 'string') throw new TypeError('capability must be a string');
    return requestCapability(cap);
  },
};

let pluginModule = null;

async function loadPlugin(source) {
  // data: URL import => no filesystem access required.
  pluginModule = await import('data:text/javascript,' + encodeURIComponent_(source));
}

async function invoke(resource, probe) {
  if (!pluginModule || typeof pluginModule.run !== 'function') {
    send({ __proto__: null, t: 'result', value: { error: 'plugin has no exported run()' } });
    return;
  }
  let value;
  try {
    value = await pluginModule.run({ resource, probe, caps });
  } catch (e) {
    value = { error: String(e && e.message ? e.message : e) };
  }
  if (!send({ __proto__: null, t: 'result', value: value ?? null })) {
    send({ __proto__: null, t: 'result', value: { error: 'plugin result is not JSON-serializable' } });
  }
}

// Informational only: the core derives the recheck verdict from its own broker.
async function recheck(cap) {
  const r = await caps.request(cap);
  send({
    __proto__: null,
    t: 'recheck_result',
    cap,
    allowed: r.allowed === true,
    reason: typeof r.reason === 'string' ? r.reason : '',
  });
}

// ---- stdin dispatch ---------------------------------------------------------
// readline fires 'line' without awaiting the handler, so load/invoke/recheck
// are SERIALIZED on a promise chain; `cap_response` is handled OUT OF BAND
// (`invoke` is parked awaiting it). Stdin content is not secret (the nonce line
// was consumed above), so the plugin observing it is accepted residual risk.
let chain = PromiseCtor.resolve();
async function handle(msg) {
  switch (msg.t) {
    case 'load':
      try {
        await loadPlugin(msg.source);
      } catch (e) {
        send({ __proto__: null, t: 'log', msg: 'load failed: ' + String(e) });
      }
      break;
    case 'invoke':
      await invoke(msg.resource, msg.probe);
      break;
    case 'recheck':
      await recheck(msg.cap);
      break;
    case 'shutdown':
      apply(exit, process, [0]);
      break;
    default:
      break;
  }
}

const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => {
  let msg;
  try {
    msg = parse(line);
  } catch {
    return;
  }
  if (!msg || typeof msg !== 'object') return;
  if (msg.t === 'cap_response') {
    const resolve = pending[msg.id];
    if (typeof resolve === 'function') {
      delete pending[msg.id];
      resolve({ allowed: msg.allowed === true, result: msg.result ?? null, reason: msg.reason });
    }
    return;
  }
  chain = chain.then(() => handle(msg));
});

send({ __proto__: null, t: 'ready' });
