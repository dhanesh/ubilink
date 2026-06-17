// Satisfies: RT-5 (binding), S1 (invariant), T1 (boundary), TN1 (capability
// broker), TN6 (isolation proof / manifold evidence E9).
// See .manifold/linking-manifesto-app.md
//
// The Node sandbox host. Runs under `node --permission` with NO fs/child/worker
// allowances, so this process — and any plugin code it loads — is denied ambient
// filesystem, child_process and worker_threads access by the Node runtime itself.
//
// It does exactly three things:
//   1. Receives the plugin SOURCE from the trusted Rust core over stdin (the core
//      read it from disk; this host has no fs rights) and loads it via a `data:`
//      URL import — which touches no filesystem, so it works with zero fs grants.
//   2. Calls the plugin's exported `run({ resource, probe, caps })`.
//   3. Exposes ONLY a `caps.request(capability)` API to the plugin. Every call is
//      forwarded over stdout to the Rust broker, which checks it against the
//      granted manifest and replies allow/deny. The plugin gets NO other handle
//      to the OS or the link graph.
//
// Protocol: newline-delimited JSON (see src-tauri/src/sandbox.rs for the spec).

import { createInterface } from 'node:readline';

// Keep stdout PURE protocol. Capture the REAL stdout writer for the host's own
// frames, then neuter the public `process.stdout.write` so untrusted plugin code
// (loaded later into this same realm) cannot forge protocol frames on stdout.
// NOTE: ambient fs/net/child denial is already enforced by `node --permission`;
// this is integrity hardening, not the security boundary. Full realm isolation
// (running the plugin in a curated vm context with no `process`) is a tracked
// hardening item for the integration-runtime task — see DECISIONS.md.
const realStderrWrite = process.stderr.write.bind(process.stderr);
const realStdoutWrite = process.stdout.write.bind(process.stdout);
console.log = (...a) => realStderrWrite('[plugin-console] ' + a.join(' ') + '\n');
console.error = console.log;
console.info = console.log;
console.warn = console.log;
// Reroute any plugin attempt to write stdout (forge a frame) to stderr instead.
process.stdout.write = (chunk, ...rest) => {
  realStderrWrite('[plugin-stdout] ' + (typeof chunk === 'string' ? chunk : String(chunk)));
  const cb = rest.find((a) => typeof a === 'function');
  if (cb) cb();
  return true;
};

function send(obj) {
  realStdoutWrite(JSON.stringify(obj) + '\n');
}

// ---- capability request plumbing -------------------------------------------
let nextId = 1;
const pending = new Map(); // id -> {resolve}

function requestCapability(cap) {
  const id = nextId++;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    send({ t: 'cap_request', id, cap });
  });
}

// The ONLY API surface handed to untrusted plugin code.
const caps = {
  // Returns { allowed, result, reason }. The plugin can never bypass this:
  // there is no fs/net/child handle in scope, and the broker is the sole arbiter.
  async request(cap) {
    if (typeof cap !== 'string') throw new TypeError('capability must be a string');
    return requestCapability(cap);
  },
};

let pluginModule = null;

async function loadPlugin(source) {
  // data: URL import => no filesystem access required, so this works even though
  // --allow-fs-read is NOT granted. The loaded code inherits the same denied
  // permission set (verified: it still cannot fs.readFile).
  const url = 'data:text/javascript,' + encodeURIComponent(source);
  pluginModule = await import(url);
}

async function invoke(resource, probe) {
  if (!pluginModule || typeof pluginModule.run !== 'function') {
    send({ t: 'result', value: { error: 'plugin has no exported run()' } });
    return;
  }
  let value;
  try {
    value = await pluginModule.run({ resource, probe, caps });
  } catch (e) {
    value = { error: String(e && e.message ? e.message : e) };
  }
  send({ t: 'result', value: value ?? null });
}

async function recheck(cap) {
  const r = await caps.request(cap);
  send({ t: 'recheck_result', cap, allowed: r.allowed, reason: r.reason });
}

// ---- stdin dispatch ---------------------------------------------------------
// readline fires 'line' without awaiting the handler, so load/invoke/recheck
// must be SERIALIZED on a promise chain (else `invoke` can run before `load`'s
// async import() resolves). `cap_response`, however, must be handled OUT OF BAND
// — `invoke` is parked awaiting it, so it cannot sit behind `invoke` in the chain
// or we'd deadlock.
let chain = Promise.resolve();
async function handle(msg) {
  switch (msg.t) {
    case 'load':
      try {
        await loadPlugin(msg.source);
      } catch (e) {
        send({ t: 'log', msg: 'load failed: ' + String(e) });
      }
      break;
    case 'invoke':
      await invoke(msg.resource, msg.probe);
      break;
    case 'recheck':
      await recheck(msg.cap);
      break;
    case 'shutdown':
      rl.close();
      process.exit(0);
      break;
    default:
      break;
  }
}

const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => {
  if (!line.trim()) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.t === 'cap_response') {
    const resolve = pending.get(msg.id);
    if (resolve) {
      pending.delete(msg.id);
      resolve({ allowed: msg.allowed, result: msg.result, reason: msg.reason });
    }
    return;
  }
  chain = chain.then(() => handle(msg));
});

send({ t: 'ready' });
