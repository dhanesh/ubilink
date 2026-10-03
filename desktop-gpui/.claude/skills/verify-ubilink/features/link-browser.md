# link-browser: Browse links through a virtualised, fuzzy-filtered list

- id: link-browser
- proven: no
- anchors: src/views/browser.rs:BrowserView, src/views/browser.rs:State, probes/link-browser.json, probes/link-browser-filter.json, probes/fixtures/graph-5000.json, probes/fixtures/graph-ranking.json, probes/fixtures/ranking-golden.json

## What it is

The link browser renders every captured resource as a row, newest-updated first,
even at 5,000 links: Phase 1 R4 requires it build at most 60 row elements and show
its first frame under 100 ms, however big the graph. A keyboard-focused filter re-
ranks the list as the user types, putting in-order subsequence matches of both the
resource's name and its URI above everything else, dropping non-matches entirely —
Phase 1 R5, re-filtering the full 5,000+ link corpus in p99 < 16 ms per keystroke.

## How to reach it

- **The browser hotkey** (`trigger_hotkey {"action": "browser"}`) or the tray's
  Open on `browser` (`open_view {"route": "browser"}`), same entry points
  `shell.md` already proves for the Phase 0 stub.
- **Typing into the filter**, which is focused the moment the browser opens
  (README's Phase 1 view-state contract) — printable keys type into it directly via
  the `key` verb, no click needed.
- **Not driven here:** moving the selection or opening a link (`browser-navigation.md`),
  marking a link (`link-marking.md`).

## Drive it

Two frozen scenarios are this feature's acceptance proof: `probes/link-browser.json`
(R4, virtualisation and the first-frame budget) and
`probes/link-browser-filter.json` (R5, all 20 golden filter queries plus the
50-keystroke p99 timing). Both run through the harness's `scenario` command; neither
is sampled — `link-browser-filter` checks every one of the 20 golden queries, not
one representative query.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name link-browser
echo "link-browser exit $?"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name link-browser-filter
echo "link-browser-filter exit $?"
```

Expected: both exit 0. `$OUT/link-browser/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "virtualised: between 1 and 60 row nodes", "row
5000 is in the tree" and "the first row is no longer built". `$OUT/link-browser-filter/result.json`'s
`verdict` is `"PASS"`; `transcript.txt` shows a PASS triplet ("top rows = the
table's", "lists only its N subsequence match(es)", "decoys absent") for each of the
20 `golden-N` blocks, plus "filter p99 < 16 ms over the 50 keystrokes".

## Proof

- **Virtualised.** `link-browser`'s "virtualised: between 1 and 60 row nodes" and
  "the list holds all 5000 links" (`size_of_set: 5000`) together prove the widget
  knows about every link while building only a fraction of the rows; "row 5000 is
  in the tree" / "the first row is no longer built" after `End` prove rows are
  recycled, not merely capped.
- **First frame.** "first frame < 100 ms (timed at present, before any a11y call)"
  reads `metrics.summary.view_first_frame.max < 100` before any `see`/`a11y` call on
  that instance.
- **Ranking, all 20 queries.** Each `golden-N` block's three steps assert the top
  rows equal `ranking-golden.json`'s `expect_top3` for that query exactly (by both
  URI and name), that only the matching count of rows exist, and that the query's
  named decoy is completely absent — the full table and the normative `rank()` rule
  it is generated from are in `probes/fixtures/README.md`.
- **Filter latency.** "no keystroke timed yet in this view" (`view.filter_samples:
  0`) proves the counter is per-open, not cumulative; "filter p99 < 16 ms over the
  50 keystrokes" then requires `view.filter_samples >= 50` and
  `view.filter_p99_ms < 16` against the 5,022-link corpus.
- **Side effect.** Both scenarios' "nothing launched" step requires the `opened`
  sink to equal `[]`.

This feature has **two** acceptance scenarios, but factory-conductor's evidence
gate reads exactly one `.verify/<instance>/link-browser/<sha>/evidence.json`
(SKILL.md's "The evidence gate" / `verify_evidence.py record`'s one-record-per-
feature-per-head shape) — recording each scenario in its own `record` call would
have the second call overwrite the first's `evidence.json` and same-named
artifacts (`result.json`, `transcript.txt`, ...), silently losing the R4 proof
behind the R5 one. Record **one** evidence set for both, with each scenario's
artifacts renamed to a unique prefix first, and `--result` derived from **both**
exit codes:

```sh
python3 "$H" doctor --instance "$INSTANCE"
REC="$WT/.verify-run/$INSTANCE/record-link-browser"
rm -rf "$REC"; mkdir -p "$REC"
RESULT=pass
for name in link-browser link-browser-filter; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature link-browser \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario link-browser + link-browser-filter (desktop-gpui/probes/{link-browser,link-browser-filter}.json) on instance $INSTANCE" \
  --observed "link-browser: $(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/link-browser/result.json"); link-browser-filter: $(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/link-browser-filter/result.json")" \
  --side-effect "opened sink stayed [] in both scenarios; see each scenario's renamed result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `scenario` replaces the instance's process and temp root before each run, so run
  `link-browser` and `link-browser-filter` as two separate `scenario` invocations
  (above), not interleaved steps of one hand-written `call`.
- Record both scenarios in **one** `verify_evidence.py record` call, not one per
  scenario (above) — each writes to the same
  `.verify/<instance>/link-browser/<sha>/`, so a second `record` to the same
  feature and head would overwrite the first's `evidence.json` and any
  same-named artifact.
- Read `metrics` for the first-frame timing before any `see`/`a11y` call on that
  instance — activating the AccessKit tree makes GPUI build it on every frame for
  the window's life, which would slow the very frame being timed. The scenario
  itself is already ordered this way; do not add an ad-hoc `see` before it if
  reproducing part of it by hand.
- The filter ranks on **both** `name` and `uri`, case-folded, NOT NFKC-normalised
  (these are identifiers, not prose) — a query that only matches one field of an
  item still ranks it; an item matching neither field is dropped entirely, never
  shown at a low rank. `probes/fixtures/README.md`'s worked examples (`gthb`,
  `rddt`, the `ab`/`aabb` minimum-window case) are the edge cases a naive "contains"
  or greedy subsequence scorer gets wrong, and the scenario's 20 queries are chosen
  to exercise them.
- Today's stub shows only its static title and has no `TextInput` or `ListItem`
  nodes at all, so both scenarios exit 1 at their first `see` until Phase 1 lands.
