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

Two frozen scenarios are this feature's acceptance proof: `probes/browser-select.json`
(R6, Up/Down/Enter) and `probes/browser-drill.json` (R7, Right/Left), both run
through the harness's `scenario` command — every key and every assertion each
scenario makes, not a hand-picked subset.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name browser-select
echo "browser-select exit $?"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name browser-drill
echo "browser-drill exit $?"
```

Expected: both exit 0. `$OUT/browser-select/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "row 1 selected at open", "row 3 carries the
selected flag", "row 2 carries the selected flag" and "the opened sink reports row
2's URI, once". `$OUT/browser-drill/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "top level: no trail", "the trail names the
parent", "the child list is the parent's connections, exactly", "top-level rows are
gone while drilled" and "row 2 carries the selected flag again".

## Proof

- **Selection moves.** `browser-select`'s "row 3 carries the selected flag" and "row
  2 carries the selected flag" each assert the `selected` flag on exactly one row
  (the other rows' `{"ne": true}` checks rule out "every row is selected" as a false
  pass).
- **Enter opens.** "nothing opened yet" (`opened` sink `== []`) immediately before
  Enter, then "the opened sink reports row 2's URI, once" (`len: 1`,
  `0.target: <row 2's URI>`) immediately after — proving Enter is what grows the
  sink, and by exactly the selected row's URI, through the core opener (S2/O4:
  automation builds record opens instead of launching anything).
- **Drill in.** `browser-drill`'s "the trail names the parent" and "the child list
  is the parent's connections, exactly" together prove Right shows the drilled-in
  view's trail and child set; "top-level rows are gone while drilled" proves the
  parent list is replaced, not merely overlaid.
- **Drill out.** "the trail is empty again" and "row 2 carries the selected flag
  again" prove Left returns to the top level with the previous selection restored,
  not just to some default.

Record each scenario separately (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
for name in browser-select browser-drill; do
  R="$OUT/$name"
  python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature browser-navigation \
    --verifier "$VERIFIER" --result pass \
    --action "scenario $name (desktop-gpui/probes/$name.json) on instance $INSTANCE" \
    --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
    --side-effect "opened sink grows by exactly the opened row's URI; see result.json" \
    $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
done
```

## Gotchas

- The filter `Input` is always focused (`link-browser.md`'s contract), so Up/Down/
  Home/End only reach the list because the browser intercepts them on a parent with
  a capture-phase action listener and stops propagation; Right/Left reach the parent
  only at the Input's text edges (bubble phase) — A11Y-NOTES.md's per-key table is
  the reference this behaviour must match, proven live in `a11y-spike.json`.
- "Reachable from the keyboard" here means the scenario never asserts a hard-coded
  focus order on its own control; it drives the same keys a user would and reads
  the `selected`/trail state back.
- `scenario` replaces the instance's process and temp root before each run, so run
  `browser-select` and `browser-drill` as two separate invocations, each seeding
  its own fresh copy of `graph-backlinks.json`.
- Today's stub has no `ListItem` rows and no trail Label at all, so both scenarios
  exit 1 at their first `see` until Phase 1 lands.
