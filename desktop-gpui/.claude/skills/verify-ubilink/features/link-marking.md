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
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0; the final `opened` line reports
`"result": []`. The link row order and A = row 1 / B = row 2 pairing follow
`probes/link-marking.json`'s own setup (`graph-backlinks.json`'s `list_links` order).
Persistence across a real process restart — the Phase 1 scenario restarts the
instance and drills into B to see A listed — is `scripts/probe.py run link-marking`'s
job (the Gotchas section below explains why this skill's `call` proves the write
itself instead).

## Proof

- **Confirmation.** An a11y `Label` reads `Linked "<A name>" ↔ "<B name>"` after the
  second Cmd-L (README's exact Tauri wording).
- **Bidirectional in the graph.** `step:{"graph": ..., "links": [[A, B]]}` reads the
  instance's own `ubilink-graph.sqlite` directly: ubilink-core stores one row per
  unordered pair and `links_of` reads either end, so this single check is the proof
  that both `links_of(A) ∋ B` and `links_of(B) ∋ A` hold, not just that *some* row
  was written.
- **Side effect.** The `opened` sink stays `[]`: marking a link never opens it.

## Gotchas

- This skill's `call` refuses the `restart` step (SKILL.md's Drive section: "`restart`
  ... need `probe.py run` itself and are refused"), so this map proves persistence by
  reading the graph file the mark wrote, in the same call, rather than by actually
  restarting the process and drilling into B afterwards. That on-disk row is exactly
  what a real restart would read back; `scripts/probe.py run link-marking` is the
  tool that drives the full restart-then-drill proof end to end.
- Plain `l` (no Cmd) must keep typing into the filter — a browser that intercepts
  bare `l` as a mark shortcut breaks `link-browser.md`'s filter contract for every
  query containing the letter.
- Today's stub has no `ListItem` rows and no Cmd-L handling at all, so this call
  fails at the first `see` until Phase 1 lands.
