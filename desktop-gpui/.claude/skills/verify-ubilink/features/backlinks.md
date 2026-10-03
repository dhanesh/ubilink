# backlinks: The capture view lists who links to what you just captured

- id: backlinks
- proven: no
- anchors: src/views/capture.rs:CaptureView, probes/backlinks.json, probes/fixtures/capture-subject.json, probes/fixtures/graph-backlinks.json, probes/fixtures/graph-backlinks-newer.json
- constraints: linking-manifesto-app:B1, linking-manifesto-app:U5

## What it is

When the captured resource already has backlinks in the graph, the capture view
lists them, newest first, right next to the capture itself — Phase 1 R3, spec U5
("backlinks are visible at the resource"). A resource with no links to it shows none;
a resource that is captured but not linked to the subject (a decoy) must never
appear.

## How to reach it

- **The capture hotkey**, same as `capture-view.md`, with a graph already seeded and
  a fixture capturing the seeded subject.
- **Not driven here:** browsing to a backlink from the list — the capture view only
  shows them, R6/R7's browser drives selecting and opening (`browser-navigation.md`).

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/backlinks"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'seed_graph:{"fixture":"graph-backlinks.json"}' \
  'step:{"graph":true,"link_count":3,"resource_count":5}' \
  'load_fixture:{"kind":"capture","path":"{fixtures}/capture-subject.json"}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture"}' \
  'see:{"role":"Label","text":"Subject Note"}' \
  'see:{"sequence":{"role":"ListItem","texts":["Gamma Doc","Beta Post","Alpha Paper"],"mode":"exact","deep":true}}' \
  'absent:{"text":"Decoy Unrelated"}' \
  'seed_graph:{"fixture":"graph-backlinks-newer.json"}' \
  'step:{"graph":true,"link_count":4,"resource_count":6}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":false,"route":null}' \
  'trigger_hotkey:{"action":"capture"}' \
  'see:{"sequence":{"role":"ListItem","texts":["Delta Memo","Gamma Doc","Beta Post","Alpha Paper"],"mode":"exact","deep":true}}' \
  'absent:{"text":"Decoy Unrelated"}' \
  opened
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0; the final `opened` line reports
`"result": []`. `seed_graph` merges a fixture into the live instance's own
`ubilink-graph.sqlite` exactly as `LinkGraph::import_json` would (probes/README.md),
so the two `step:{"graph":...}` checks read back the merge immediately, before the
view ever reopens.

## Proof

- **Exactly the seeded backlinks, newest first.** The `ListItem` sequence (`deep:
  true`, since `List` puts a row's text in a `Label` child — A11Y-NOTES.md) matches
  the three backlinks' names/URIs in recency order and nothing else; the decoy, which
  the fixture captures but never links to the subject, is absent from the view both
  before and after the new link lands.
- **Re-query on reopen.** A fourth, newer backlink seeded into the graph while the
  panel is open does not appear until the capture view is closed and reopened — the
  reopen is the proof that the view re-reads the graph rather than caching its first
  read.
- **Side effect.** The `opened` sink stays `[]`.

## Gotchas

- `seed_graph` is setup, not proof — it stands in for links an earlier capture/mark
  action would have created; the Drive section's own graph reads back confirm the
  merge happened before asserting anything about the view.
- The sequence query's `mode: "exact"` means exactly that many `ListItem`s must
  exist, in that order — a view that renders the decoy anywhere, or in the wrong
  order, fails here even if the three real backlinks are all present.
- Matching on **both** URI and name (two separate sequence assertions in the Phase 1
  scenario, `probes/backlinks.json`) is what catches a view that gets the order right
  by URI but renders a stale or swapped name alongside it; this map's command checks
  names only to keep the command short, but a full proof checks both.
- Today's stub shows only its static title and never lists anything, so this call
  fails at the first backlink `see` until Phase 1 lands.
