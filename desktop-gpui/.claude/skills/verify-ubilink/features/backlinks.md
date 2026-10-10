# backlinks: The capture view lists who links to what you just captured

- id: backlinks
- proven: no
- anchors: probes/backlinks.json, probes/backlink-open.json, probes/fixtures/capture-subject.json, probes/fixtures/graph-backlinks.json, probes/fixtures/graph-backlinks-newer.json
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
SCENARIOS="backlinks backlink-open"
for name in $SCENARIOS; do
  python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name "$name"
  echo "$name exit $?"
done
```

Expected: both exit 0. `$OUT/backlinks/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS lines for "exactly the 3 backlinks, newest first (URIs
and names)", "the decoy is not listed" (twice — before and after the fourth
backlink lands), and "the newer backlink comes first, then the other three".

`$OUT/backlink-open/result.json`'s `verdict` is `"PASS"`; `transcript.txt` shows PASS
for "copy: pbpaste prints the subject URI, change count higher", "open: Tab moves
focus onto the Beta Post backlink row", "open: the opened sink holds exactly the Beta
Post URI" and "open: not copied (pasteboard still the nonce, change count
unchanged)".

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
- **Open a backlink (Phase 2 parity, `backlink-open`).** With no row focused, Enter
  still copies the captured URI and opens nothing. After a `restart`, Tab moves
  keyboard focus onto the second backlink row (its `ListItem` carries `focused`,
  exactly one row focused) and Enter opens exactly that backlink's URI through the
  core opener (`opened` sink length 1) without touching the pasteboard (nonce, change
  count unchanged).

Record it (SKILL.md, Evidence). Each feature has one evidence record per head, so
both scenarios go into **one** `verify_evidence.py record` call, each scenario's
artifacts renamed to a unique prefix first, and `--result` is `fail` if **either**
scenario's exit is not 0 (browser-navigation.md explains why).

```sh
python3 "$H" doctor --instance "$INSTANCE"
mkdir -p "$WT/.verify-run/$INSTANCE"
REC="$(mktemp -d "$WT/.verify-run/$INSTANCE/record-backlinks.XXXXXX")"
RESULT=pass
for name in $SCENARIOS; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature backlinks \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario(s) $SCENARIOS (desktop-gpui/probes/<name>.json) on instance $INSTANCE" \
  --observed "$(for name in $SCENARIOS; do printf '%s: %s; ' "$name" "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/$name/result.json")"; done)" \
  --side-effect "opened sink stayed [] in backlinks and in backlink-open's copy round; it holds exactly the focused backlink's URI after its open round; see each scenario's renamed result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
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
- `backlink-open` is a Phase 2 parity scenario: on the Phase 1 head (5017f5b) it
  exits 1 at "open: Tab moves focus onto the Beta Post backlink row" (Tab focuses the
  Copy link button, the rows are not focusable) until Phase 2 lands. The row's own
  `ListItem` node must be the focused (or active-descendant) node.
