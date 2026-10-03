# graph-data: Export and import round-trip the graph; Reset asks first

- id: graph-data
- proven: no
- anchors: src/views/settings.rs:SettingsView, probes/graph-roundtrip.json, probes/graph-reset.json, probes/fixtures/graph-backlinks.json, probes/fixtures/graph-empty.json
- constraints: linking-manifesto-app:T2

## What it is

Settings can export the whole graph to a JSON file and import it back with no
change (Phase 1 R11), and can reset the graph to empty — but only after an explicit
confirmation step, never on the first press (Phase 1 R12). A stats line
(exactly `"<n> links · <n> connections"`) is re-read every time Settings is shown,
so it reflects whatever the graph holds right now, in-process edit or out-of-band
seed alike.

## How to reach it

- **Tab to Export/Import/Reset, then Enter** (the scenario's `key_until` step, then
  `key {"keys": "enter"}`).
- **Not driven here:** the launch-at-login switch (`launch-at-login.md`), adapter
  switches (`settings-adapters.md`) — same view, different controls.

## Drive it

Two frozen scenarios are this feature's acceptance proof: `probes/graph-roundtrip.json`
(R11, export then import with no change) and `probes/graph-reset.json` (R12,
confirmation before Reset), both run through the harness's `scenario` command.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name graph-roundtrip
echo "graph-roundtrip exit $?"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name graph-reset
echo "graph-reset exit $?"
```

Expected: both exit 0. `$OUT/graph-roundtrip/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "a11y counts", "exported to the fixture path as
ubilink.graph.v3", "its link set equals the seed", "a11y counts read 0" and "a11y
counts equal the original". `$OUT/graph-reset/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "no confirmation before Reset", "confirmation
shown: question, Delete everything, Cancel", "asking deleted nothing", "Cancel left
graph_stats unchanged", "Confirm: graph_stats report 0" and "the a11y count reads
0".

## Proof

- **Export preserves the link set.** `graph-roundtrip`'s "its link set equals the
  seed" reads the export file back (`graph: true, export: true`) and requires the
  same resource/link counts the live graph had.
- **Reset asks first.** `graph-reset`'s "no confirmation before Reset" requires no
  confirmation `Label`/buttons exist before Reset is pressed; "confirmation shown"
  requires the question and both buttons after pressing it; "asking deleted
  nothing" and "Cancel left graph_stats unchanged" require the graph untouched
  until "Delete everything" is actually pressed.
- **No role asserted on the confirmation surface itself** — gpui-component's
  `Dialog` container never reaches AccessKit in this gpui-pre version
  (A11Y-NOTES.md); the scenario's proof is the question text and the two buttons,
  never a `Dialog` role.
- **Import restores exactly.** `graph-roundtrip`'s "graph_stats equal the original"
  and "a11y counts equal the original" require both the counts and the stats line
  to return to the pre-export values after emptying the graph and importing.
- **Stats line is exact and re-read every show.** Both scenarios assert the stats
  `Label` reads exactly `"5 links · 3 connections"` or `"0 links · 0 connections"`
  (`exact: true`), the frozen scenarios' own wording — re-asserted fresh after every
  graph change, proving Settings does not cache it.
- **Side effect.** Both scenarios' "nothing launched" step requires the `opened`
  sink to equal `[]`.

Record each scenario separately (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
for name in graph-roundtrip graph-reset; do
  R="$OUT/$name"
  python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature graph-data \
    --verifier "$VERIFIER" --result pass \
    --action "scenario $name (desktop-gpui/probes/$name.json) on instance $INSTANCE" \
    --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
    --side-effect "opened sink stayed []; see result.json" \
    $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
done
```

## Gotchas

- The stats text is exactly `"<n> links · <n> connections"` — **not**
  `"<n> link(s) · <n> connection(s)"` (an earlier draft of this map assumed the
  parenthetical-plural wording from the README's prose description; the frozen
  scenarios use the plain plural, and the scenario's literal text wins).
- Import **merges** into the live graph (`LinkGraph::import_json`'s own semantics);
  `graph-roundtrip`'s `seed_graph ... replace: true` between export and import is
  what makes "restores exactly" a real emptying, not just "adds more" — not Reset
  (that is `graph-reset`'s own proof).
- `graph-empty.json` alone, imported without first clearing, would change nothing —
  it is not itself a reset.
- `scenario` replaces the instance's process and temp root before each run, so run
  `graph-roundtrip` and `graph-reset` as two separate invocations.
- Today's stub has no stats Label and no Export/Import/Reset buttons at all, so
  both scenarios exit 1 at their first `see` until Phase 1 lands.
