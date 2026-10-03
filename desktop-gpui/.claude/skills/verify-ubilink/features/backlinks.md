# backlinks: The capture view lists who links to what you just captured

- id: backlinks
- proven: no
- anchors: probes/backlinks.json, probes/fixtures/capture-subject.json, probes/fixtures/graph-backlinks.json, probes/fixtures/graph-backlinks-newer.json
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

The acceptance proof is the frozen scenario `probes/backlinks.json`, run with the
harness's `scenario` command. It seeds `graph-backlinks.json`, captures the subject,
asserts the exact backlink set and order, merges a fourth newer backlink, and
asserts the reopened view's new order — every assertion the Phase 1 scenario makes,
not a hand-picked subset.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name backlinks
echo "exit $?"
```

Expected: exit 0. `$OUT/backlinks/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS lines for "exactly the 3 backlinks, newest first (URIs
and names)", "the decoy is not listed" (twice — before and after the fourth
backlink lands), and "the newer backlink comes first, then the other three".

## Proof

- **Exactly the seeded backlinks, newest first.** The scenario's sequence queries
  (`mode: "exact"`, `deep: true`) match both the three backlinks' URIs and their
  names, in recency order, with nothing else — a view that gets the order right by
  URI but shows a stale or swapped name fails the second sequence query even if the
  first passes.
- **Decoy never appears.** "the decoy is not listed" is asserted both before and
  after the fourth backlink is seeded.
- **Re-query on reopen.** The fourth, newer backlink is seeded while the panel is
  open; the scenario closes and reopens the capture view (two more `trigger_hotkey`
  calls) before asserting the new order — proving the view re-reads the graph
  rather than caching its first read.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/backlinks"
RESULT=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print("pass" if r["exit"]==0 else "fail")' "$R/result.json")
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature backlinks \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario backlinks (desktop-gpui/probes/backlinks.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "opened sink stayed []; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `scenario` replaces the instance's process and temp root with a fresh one before
  it runs, so any graph state from an earlier `call` on this instance is gone —
  the scenario's own `setup.graph` is what seeds it.
- The sequence query's `mode: "exact"` means exactly that many `ListItem`s must
  exist, in that order — a view that renders the decoy anywhere, or in the wrong
  order, fails here even if the three real backlinks are all present.
- Today's stub shows only its static title and never lists anything, so `scenario
  --name backlinks` exits 1 at the first backlink assertion until Phase 1 lands.
