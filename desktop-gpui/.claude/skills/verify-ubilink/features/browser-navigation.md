# browser-navigation: Arrow keys select, Enter opens, Right drills in

- id: browser-navigation
- proven: no
- anchors: probes/browser-select.json, probes/browser-drill.json, probes/fixtures/graph-backlinks.json
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

This feature has **two** acceptance scenarios, but factory-conductor's evidence
gate reads exactly one `.verify/<instance>/browser-navigation/<sha>/evidence.json`
(SKILL.md's "The evidence gate" / `verify_evidence.py record`'s one-record-per-
feature-per-head shape) — recording each scenario in its own `record` call would
have the second call overwrite the first's `evidence.json` and same-named
artifacts (`result.json`, `transcript.txt`, ...), silently losing the R6 proof
behind the R7 one. Record **one** evidence set for both, with each scenario's
artifacts renamed to a unique prefix first, and `--result` derived from **both**
exit codes:

```sh
python3 "$H" doctor --instance "$INSTANCE"
REC="$WT/.verify-run/$INSTANCE/record-browser-navigation"
rm -rf "$REC"; mkdir -p "$REC"
RESULT=pass
for name in browser-select browser-drill; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature browser-navigation \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario browser-select + browser-drill (desktop-gpui/probes/{browser-select,browser-drill}.json) on instance $INSTANCE" \
  --observed "browser-select: $(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/browser-select/result.json"); browser-drill: $(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/browser-drill/result.json")" \
  --side-effect "opened sink grows by exactly the opened row's URI in browser-select; nothing opened in browser-drill; see each scenario's renamed result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- Record both scenarios in **one** `verify_evidence.py record` call, not one per
  scenario (above) — each writes to the same
  `.verify/<instance>/browser-navigation/<sha>/`, so a second `record` to the same
  feature and head would overwrite the first's `evidence.json` and any
  same-named artifact.

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
