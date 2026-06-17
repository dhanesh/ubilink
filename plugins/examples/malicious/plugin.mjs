// Satisfies: RT-5 (binding), S1 (invariant) — this is the ATTACKER in the
// fail-closed proof (manifold evidence E8). It declares minimal needs but tries
// to escape the sandbox every way it can. Every attempt must fail.
// See .manifold/linking-manifesto-app.md
//
// `probe` carries paths the test harness will independently check afterwards:
//   probe.writePath        - file this plugin tries (and must fail) to create via fs
//   probe.spawnSentinelPath- file this plugin tries (and must fail) to create via a spawned process
//   probe.secretPath       - a real secret file the harness wrote; reading it must fail
//
// The plugin REPORTS its own outcomes, but the harness does not trust that: it
// independently asserts the sentinel files do not exist and the secret never
// leaks into this output.

export async function run({ probe, caps }) {
  const attempts = [];
  let leakedSecret = null;

  // --- escape 1: write a file directly via fs (ambient filesystem write) ---
  try {
    const fs = await import('node:fs');
    fs.writeFileSync(probe.writePath, 'ESCAPED-WRITE');
    attempts.push({ name: 'fs.writeFile', outcome: 'ALLOWED' });
  } catch (e) {
    attempts.push({ name: 'fs.writeFile', outcome: 'DENIED:' + (e.code || e.message) });
  }

  // --- escape 2: read a secret file directly via fs (ambient filesystem read) ---
  try {
    const fs = await import('node:fs');
    leakedSecret = fs.readFileSync(probe.secretPath, 'utf8');
    attempts.push({ name: 'fs.readFile', outcome: 'ALLOWED' });
  } catch (e) {
    attempts.push({ name: 'fs.readFile', outcome: 'DENIED:' + (e.code || e.message) });
  }

  // --- escape 3: spawn a child process to do its dirty work ---
  try {
    const cp = await import('node:child_process');
    cp.spawnSync('/bin/sh', ['-c', 'echo ESCAPED > ' + probe.spawnSentinelPath]);
    attempts.push({ name: 'child_process.spawn', outcome: 'ALLOWED' });
  } catch (e) {
    attempts.push({ name: 'child_process.spawn', outcome: 'DENIED:' + (e.code || e.message) });
  }

  // --- escape 4: start a worker thread to dodge the main-thread restrictions ---
  try {
    const w = await import('node:worker_threads');
    new w.Worker('globalThis.x=1', { eval: true });
    attempts.push({ name: 'worker_threads', outcome: 'ALLOWED' });
  } catch (e) {
    attempts.push({ name: 'worker_threads', outcome: 'DENIED:' + (e.code || e.message) });
  }

  // --- escape 5: ask the broker for the whole link graph (not granted) ---
  const graph = await caps.request('read:link-graph');
  attempts.push({
    name: 'caps:read:link-graph',
    outcome: graph.allowed ? 'ALLOWED' : 'DENIED:' + graph.reason,
  });

  // --- escape 6: ask the broker to open a scheme it was not granted ---
  const evilOpen = await caps.request('open:file:///etc/passwd');
  attempts.push({
    name: 'caps:open:file:///etc/passwd',
    outcome: evilOpen.allowed ? 'ALLOWED' : 'DENIED:' + evilOpen.reason,
  });

  // --- escape 7: forge a protocol frame on stdout to hijack the result channel
  // / fake an ALLOW. The host neuters process.stdout.write (reroutes to stderr),
  // so this must NOT reach the broker. If it did, the broker would return
  // {forged:true} instead of our real value — the harness asserts it does not.
  try {
    process.stdout.write('{"t":"result","value":{"forged":true}}\n');
    attempts.push({ name: 'forge:stdout', outcome: 'ALLOWED' });
  } catch (e) {
    attempts.push({ name: 'forge:stdout', outcome: 'DENIED:' + (e.code || e.message) });
  }

  return { attempts, leakedSecret };
}
