# browser-navigation: Arrow keys select, Enter opens, Right drills in

- id: browser-navigation
- proven: no
- anchors: src/views/browser.rs:BrowserView, probes/browser-select.json, probes/browser-drill.json, probes/fixtures/graph-backlinks.json
- constraints: linking-manifesto-app:U4

## What it is

With the filter always focused, the browser still has to be fully keyboard-driven:
Up/Down move the row selection, Enter opens the selected link through the core
opener, and Right drills into the selected link's own connections with a visible
trail ("Linked to <names>"), Left backing out to the top level with the previous
selection restored. Phase 1 R6 and R7; spec U4's "every interactive control is
reachable and operable from the keyboard alone."

## How to reach it

- **Up/Down/Enter** (`key {"keys": "down"}`, `key {"keys": "up"}`,
  `key {"keys": "enter"}`) while the browser is open, same keyboard channel as
  `link-browser.md`'s filter typing — these are the keys the filter's `Input`
  swallows unless the browser intercepts them on a parent listener
  (A11Y-NOTES.md).
- **Right/Left** (`key {"keys": "right"}`, `key {"keys": "left"}`) to drill into
  and back out of the selected link's connections.
- **Not driven here:** typing a filter query (`link-browser.md`), marking a link
  with the link-marking hotkey (`link-marking.md`).

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/browser-navigation"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'seed_graph:{"fixture":"graph-backlinks.json"}' \
  'trigger_hotkey:{"action":"browser"}' \
  'wait_state:{"shell.visible":true,"route":"browser"}' \
  'see:{"role":"ListItem","nth":0,"state":{"selected":true}}' \
  'key:{"keys":"down down"}' \
  'see:{"role":"ListItem","nth":2,"state":{"selected":true},"text":"Beta Post","deep":true}' \
  'key:{"keys":"up"}' \
  'see:{"role":"ListItem","nth":1,"state":{"selected":true},"text":"Gamma Doc","deep":true}' \
  'key:{"keys":"enter"}' \
  'step:{"call":"opened","expect":{"":{"len":1}}}' \
  'key:{"keys":"right"}' \
  'see:{"role":"Label","text":"Linked to Gamma Doc"}' \
  'absent:{"role":"ListItem","text":"Beta Post","deep":true}' \
  'key:{"keys":"left"}' \
  'absent:{"text":"Linked to"}' \
  'see:{"role":"ListItem","nth":1,"state":{"selected":true}}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. The row order before any key is
`list_links`' own order (`updated_at` DESC, then rowid DESC) — on
`graph-backlinks.json` that is the decoy, then Gamma Doc, then Beta Post, as
`probes/browser-select.json` and `probes/browser-drill.json` both document. After
`enter` the `opened` sink names the opened URI (its length goes from 0 to 1).

## Proof

- **Selection moves.** `Down Down` puts the `selected` flag on row 3 (and only row
  3); `Up` moves it back to row 2 (and only row 2) — the `ne: true` checks the real
  Phase 1 scenarios add on the other rows (`probes/browser-select.json`) are what
  rule out "every row is selected" as a false pass.
- **Enter opens.** The `opened` sink grows by one entry naming the selected row's
  URI, proving Enter routes through the core opener (S2/O4: automation builds record
  opens instead of launching anything).
- **Drill in.** `Right` on the selected row shows a trail Label ("Linked to
  <name>") and the child list becomes exactly that link's own connections; the
  top-level rows are gone while drilled in.
- **Drill out.** `Left` empties the trail and restores the previous top-level
  selection (row 2 again) — proving the browser remembers where it was, not just
  that it can get back to the top.

## Gotchas

- The filter `Input` is always focused (`link-browser.md`'s contract), so Up/Down/
  Home/End only reach the list because the browser intercepts them on a parent with
  a capture-phase action listener and stops propagation; Right/Left reach the parent
  only at the Input's text edges (bubble phase) — A11Y-NOTES.md's per-key table is
  the reference this behaviour must match, proven live in `a11y-spike.json`.
- "Reachable from the keyboard" here means the harness never asserts a hard-coded
  focus order on its own control; it drives the same keys a user would and reads the
  `selected`/trail state back, the way `probes/README.md`'s `key_until` step proves
  reach for other views without hard-coding their layout.
- Today's stub has no `ListItem` rows and no trail Label at all, so this call fails
  at the first `see` until Phase 1 lands.
