// Satisfies: SC-3 (revocable grants), TN1 (capability broker) — test fixture.
//
// A plugin host that speaks the authenticated frame protocol correctly but LIES
// in `recheck_result` (always claims allowed=true). It models a plugin that has
// subverted host logic it shares a realm with (without learning the nonce is
// not even required here). The core must derive the recheck verdict from its
// OWN broker decision, never from what the host reports.
// Used by core/tests/sandbox.rs::recheck_verdict_comes_from_the_broker.

import fs from 'node:fs';
import { createInterface } from 'node:readline';

const buf = Buffer.alloc(65);
let off = 0;
while (off < buf.length) off += fs.readSync(0, buf, off, buf.length - off, null);
const NONCE = buf.toString('latin1', 0, 64);
const send = (o) => fs.writeSync(1, NONCE + ' ' + JSON.stringify(o) + '\n');

let pendingCap = null;
createInterface({ input: process.stdin }).on('line', (line) => {
  const m = JSON.parse(line);
  if (m.t === 'recheck') {
    if (m.cap.startsWith('unbrokered:')) {
      // Claim a verdict without ever asking the broker.
      send({ t: 'recheck_result', cap: m.cap, allowed: true, reason: 'trust me' });
      return;
    }
    pendingCap = m.cap;
    send({ t: 'cap_request', id: 1, cap: m.cap });
  } else if (m.t === 'cap_response') {
    // The broker said no; report yes anyway.
    send({ t: 'recheck_result', cap: pendingCap, allowed: true, reason: 'trust me' });
  } else if (m.t === 'shutdown') {
    process.exit(0);
  }
});
send({ t: 'ready' });
