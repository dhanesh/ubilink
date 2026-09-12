// Satisfies: S1 (invariant), S5 (DoS bounds), TN1 (capability broker) — ATTACKER
// fixture for the sandbox-hardening proofs in src-tauri/tests/sandbox.rs.
// See DECISIONS.md D1/D3/D4 and plugins/README.md.
//
// `probe.mode` selects one attack. The harness never trusts this plugin's
// self-report for the security verdict: it asserts on what the CORE accepted
// (e.g. the result it returned) or on OS-observable facts (files, processes).

import fs from 'node:fs';

// A frame that, if the core accepted it, would replace the plugin's real result.
const FORGED = '{"t":"result","value":{"forged":true}}';
// Candidate session secrets: the host's frame nonce is 64 lowercase hex chars.
const HEX64 = /[0-9a-f]{64}/g;

// Keep the event loop alive forever, so the process never exits on its own:
// only a kill from the core can end it (proves Drop kills + reaps).
setInterval(() => {}, 1 << 30);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function outcome(f) {
  try {
    const v = f();
    return 'ALLOWED' + (v === undefined ? '' : ':' + String(v));
  } catch (e) {
    return 'DENIED:' + (e.code || e.message);
  }
}

// Write a forged frame on fd 1 with each candidate secret as a prefix, plus bare.
function forgeWith(candidates) {
  const lines = [FORGED];
  for (const c of candidates) lines.push(c + ' ' + FORGED);
  for (const l of lines) {
    try {
      fs.writeSync(1, l + '\n');
    } catch {
      /* ignore */
    }
  }
  return lines.length;
}

async function forge(vector, caps) {
  const notes = { vector };
  switch (vector) {
    // 1. Bypass the host's process.stdout.write stub via the stream prototype.
    case 'proto-write':
      Object.getPrototypeOf(process.stdout).write.call(process.stdout, FORGED + '\n');
      break;
    // 2. Write straight to fd 1 (fd writes are not gated by --permission).
    case 'fd-write':
      fs.writeSync(1, FORGED + '\n');
      break;
    // 3. Replace the global JSON.stringify so the HOST's own send() emits our frame.
    case 'stringify': {
      const orig = JSON.stringify;
      JSON.stringify = () => FORGED;
      caps.request('read:focused-resource'); // host serializes a frame now
      await sleep(50);
      JSON.stringify = orig;
      break;
    }
    // 4. Pollute Object.prototype.toJSON so the host's frame objects serialize as ours.
    case 'tojson': {
      // eslint-disable-next-line no-extend-native
      // Null-prototype replacement objects, so serializing them does not
      // recurse back into this polluted toJSON.
      Object.prototype.toJSON = function () {
        const value = Object.create(null);
        value.forged = true;
        const frame = Object.create(null);
        frame.t = 'result';
        frame.value = value;
        return frame;
      };
      caps.request('read:focused-resource');
      await sleep(50);
      delete Object.prototype.toJSON;
      break;
    }
    // 5. Scrape the V8 heap for the session nonce, then forge with every candidate.
    case 'heap': {
      let candidates = [];
      try {
        let text = '';
        for await (const chunk of builtin('v8').getHeapSnapshot()) text += chunk;
        candidates = [...new Set(text.match(HEX64) || [])];
        notes.heap = 'ALLOWED:' + candidates.length;
      } catch (e) {
        notes.heap = 'DENIED:' + (e.code || e.message);
      }
      notes.forged = forgeWith(candidates);
      break;
    }
    // 6. Look for the nonce in argv / env / everything the plugin can read on stdin.
    case 'scan': {
      let seen = '';
      process.stdin.on('data', (d) => {
        seen += String(d);
      });
      caps.request('read:focused-resource'); // provoke a core->host message
      await sleep(300);
      const hay = [process.argv.join(' '), process.execArgv.join(' '), JSON.stringify(process.env), seen].join('\n');
      const candidates = [...new Set(hay.match(HEX64) || [])];
      notes.candidates = candidates.length;
      notes.forged = forgeWith(candidates);
      break;
    }
    default:
      throw new Error('unknown forge vector ' + vector);
  }
  await sleep(50); // let any forged frame reach the core before the real result
  return { real: true, notes };
}

// Synchronous access to a builtin module from inside an ESM data: module.
const builtin = (name) => process.getBuiltinModule(name);

export async function run({ probe, caps }) {
  switch (probe.mode) {
    case 'forge':
      return forge(probe.vector, caps);

    // Environment disclosure: what did we inherit from the core?
    case 'env': {
      let secret = null;
      const readSecret = outcome(() => {
        secret = fs.readFileSync(probe.secretPath, 'utf8');
      });
      return {
        keys: Object.keys(process.env),
        planted: process.env[probe.plantedVar] ?? null,
        nodeOptions: process.env.NODE_OPTIONS ?? null,
        fsReadGranted: process.permission ? process.permission.has('fs.read') : null,
        readSecret,
        secret,
        cwd: process.cwd(),
      };
    }

    // Signals: can we probe/signal the core (our parent)?
    case 'signal':
      return { killParent: outcome(() => process.kill(process.ppid, 0)) };

    // Native file writes that bypass the Node permission model.
    case 'native-write': {
      const trace = outcome(() => {
        const t = builtin('trace_events').createTracing({ categories: ['node.perf'] });
        t.enable();
        t.disable();
      });
      const report = outcome(() => process.report.writeReport(probe.reportPath));
      return { trace, report };
    }

    // DoS: never finish, but keep sending legitimate traffic just under any
    // per-message timeout so a per-message deadline never fires.
    case 'chatty':
      for (;;) {
        await caps.request('read:focused-resource');
        await sleep(100);
      }

    // DoS: synchronously flood cap_requests WITHOUT ever yielding, so the host
    // never reads stdin; the core's replies fill the pipe and its write blocks.
    case 'flood':
      for (;;) caps.request('read:focused-resource');

    default:
      throw new Error('unknown mode ' + probe.mode);
  }
}
