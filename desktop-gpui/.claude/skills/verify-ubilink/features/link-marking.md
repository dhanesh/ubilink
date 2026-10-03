# link-marking: Cmd-L marks two links and connects them both ways

- id: link-marking
- proven: no
- anchors: src/views/browser.rs:BrowserView, probes/link-marking.json, probes/fixtures/graph-backlinks.json
- constraints: linking-manifesto-app:T2

## What it is

Pressing Cmd-L on one link, moving to a second, and pressing Cmd-L again links them
both ways in the graph and shows a confirmation naming both. Phase 1 R8, matching
the Tauri browser's own Cmd-L binding (plain `L` still types into the always-focused
filter — spec ruling R-n).

## How to reach it

- **Cmd-L** (`key {"keys": "cmd-l"}`) on the selected row, twice (select A, mark,
  select B, mark). Cmd-L is not a bound action the filter `Input` consumes — it has
  no simulated `key_char` (a platform modifier), so it reaches the browser's own
  `on_key_down` listener directly (A11Y-NOTES.md).
- **Not driven here:** typing `l` (lowercase, unmodified) into the filter, which is
  `link-browser.md`'s territory, not a mark action.

## Drive it

Two commands: a `call` that marks A then B, then a real `restart` (SKILL.md's
"Proving persistence" section) followed by a second `call` that reopens the browser
and drills into B to see A listed among its connections — proving the link was
written to the graph, not held only in the view's in-memory state.

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/link-marking"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'seed_graph:{"fixture":"graph-backlinks.json"}' \
  'step:{"graph":true,"no_links":[["https://decoy.example/unrelated","https://gamma.example/three"]]}' \
  'trigger_hotkey:{"action":"browser"}' \
  'wait_state:{"shell.visible":true,"route":"browser"}' \
  'see:{"role":"ListItem","nth":0,"state":{"selected":true},"text":"Decoy Unrelated","deep":true}' \
  'key:{"keys":"cmd-l"}' \
  'key:{"keys":"down"}' \
  'key:{"keys":"cmd-l"}' \
  'see:{"role":"Label","text":"Linked “Decoy Unrelated” ↔ “Gamma Doc”"}' \
  'step:{"graph":true,"links":[["https://decoy.example/unrelated","https://gamma.example/three"]],"link_count":4}' \
  opened
echo "mark exit $?"

python3 "$H" restart --instance "$INSTANCE" --verifier "$VERIFIER"
echo "restart exit $?"

python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'trigger_hotkey:{"action":"browser"}' \
  'wait_state:{"shell.visible":true,"route":"browser"}' \
  'see:{"role":"ListItem","nth":1,"state":{"selected":true},"text":"Gamma Doc","deep":true}' \
  'key:{"keys":"right"}' \
  'see:{"role":"ListItem","text":"Decoy Unrelated","deep":true}'
echo "drill-after-restart exit $?"
```

Expected: every `call` prints `"ok": true` lines and exits 0; `restart` prints
`{"RESTARTED": {..., "data_dir_kept": true, ...}}` and exits 0 (`seed_graph` wrote
straight to the instance's `ubilink-graph.sqlite`, not to the app's in-memory state,
so nothing needs replaying for the graph itself to survive — only `load_fixture`
kinds are replayed). The link row order and A = row 1 / B = row 2 pairing follow
`probes/link-marking.json`'s own setup (`graph-backlinks.json`'s `list_links`
order); after the restart, row 2 ("Gamma Doc", B) is selected again (`list_links`
order is unchanged) and drilling right lists "Decoy Unrelated" (A) among its
connections, in addition to the connection the fixture already seeded.

## Proof

- **Confirmation.** An a11y `Label` reads `Linked "<A name>" ↔ "<B name>"` after the
  second Cmd-L (README's exact Tauri wording).
- **Bidirectional in the graph.** `step:{"graph": ..., "links": [[A, B]]}` reads the
  instance's own `ubilink-graph.sqlite` directly: ubilink-core stores one row per
  unordered pair and `links_of` reads either end, so this single check is the proof
  that both `links_of(A) ∋ B` and `links_of(B) ∋ A` hold, not just that *some* row
  was written.
- **Survives a real restart.** After the mark and a real `restart` (same binary,
  same data dir), reopening the browser and drilling into B lists A among its
  connections — proof the link lives in the graph the view reads on every open, not
  only in whatever state the view held before the restart.
- **Side effect.** The `opened` sink stays `[]`: marking a link never opens it.

## Gotchas

- `restart` (SKILL.md's "Proving persistence" section) is a harness command run
  between two `call`s, not a step inside `call` — `call` itself still refuses a
  `restart` step. It keeps the temp root (so the instance's `ubilink-graph.sqlite`
  survives untouched) and relaunches the same binary.
- Plain `l` (no Cmd) must keep typing into the filter — a browser that intercepts
  bare `l` as a mark shortcut breaks `link-browser.md`'s filter contract for every
  query containing the letter.
- Today's stub has no `ListItem` rows and no Cmd-L handling at all, so this call
  fails at the first `see` until Phase 1 lands.
