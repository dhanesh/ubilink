// Satisfies: RT-6, B3 (goal), TN1 (capability broker), S1 (invariant).
// See .manifold/linking-manifesto-app.md
//
// A well-behaved example integration. It only uses capabilities it declared in
// its manifest (needs: ["open:obsidian://", "read:focused-resource"]). It also
// tries ONE thing it did NOT declare (read:link-graph) to demonstrate that even
// a benign plugin is held to deny-by-default.

export async function run({ caps }) {
  // 1. Granted: open a link via the obsidian scheme. The broker performs the
  //    actual OS open; the plugin never touches the OS directly.
  const open = await caps.request('open:obsidian://vault/Note.md');

  // 2. Granted: read the single focused resource (and ONLY that resource).
  const focused = await caps.request('read:focused-resource');

  // 3. NOT granted: attempt to read the whole link graph -> expect deny.
  const graph = await caps.request('read:link-graph');

  return {
    openAllowed: open.allowed,
    openResult: open.result,
    focusedAllowed: focused.allowed,
    focusedResource: focused.result, // a single resource, never a collection
    graphAllowed: graph.allowed, // must be false
    graphReason: graph.reason,
  };
}
