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

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/link-browser"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'seed_graph:{"fixture":"graph-5000.json"}' \
  'step:{"graph":true,"resource_count":5000,"link_count":0}' \
  'trigger_hotkey:{"action":"browser"}' \
  'wait_state:{"shell.visible":true,"route":"browser"}' \
  'step:{"call":"metrics","retry_ms":3000,"expect":{"summary.view_first_frame.count":{"ge":1},"summary.view_first_frame.max":{"lt":100}}}' \
  'see:{"role":"ListItem","min_count":1,"max_count":60}' \
  'see:{"role":"ListItem","nth":0,"state":{"size_of_set":5000}}' \
  'key:{"keys":"end"}' \
  'see:{"role":"ListItem","nth":0,"state":{"size_of_set":5000}}' \
  opened
echo "exit $?"
```

```sh
OUT2="$WT/.verify-run/$INSTANCE/captures/link-browser-filter"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT2" \
  'seed_graph:{"fixture":"graph-ranking.json"}' \
  'step:{"graph":true,"resource_count":22,"link_count":0}' \
  'trigger_hotkey:{"action":"browser"}' \
  'see:{"role":"TextInput"}' \
  'see:{"role":"ListItem","count":22}' \
  'key:{"keys":"g t h b"}' \
  'see:{"role":"ListItem","nth":0,"text":"GitHub - ubilink","deep":true}' \
  'see:{"role":"ListItem","count":1}' \
  'absent:{"role":"ListItem","text":"gitlab.com/ubilink/ubilink-mirror","deep":true}' \
  'key:{"keys":"backspace backspace backspace backspace"}' \
  'see:{"role":"ListItem","count":22}' \
  opened
echo "exit $?"
```

Expected: every step in both calls prints `"ok": true`, exit 0; `opened` reports
`"result": []`. The ranking call's `gthb` round is golden query 1 of the 20 in
`probes/fixtures/ranking-golden.json`; the Phase 1 scenario
(`probes/link-browser-filter.json`) runs all 20, plus a 50-keystroke timing script
against the 5,022-link corpus checking `view.filter_p99_ms < 16`.

## Proof

- **Virtualised.** At 5,000 links, between 1 and 60 `ListItem` nodes exist at once;
  the first row's `size_of_set` reads 5000 (the whole list is known to the widget
  even though most rows are never built), and `End` brings row 5000 into the tree
  while the first row drops out — proving rows are built and recycled, not all kept.
- **First frame.** `metrics.summary.view_first_frame.max` is read before any a11y
  call (activating the tree would slow every later frame) and must be under 100 ms.
- **Ranking.** For each golden query, the top matching rows equal
  `ranking-golden.json`'s `expect_top3` for that query, in order, and all the fixture's
  decoys for that query are completely absent — not merely ranked low. The full table
  and the normative `rank()` rule it is generated from are in
  `probes/fixtures/README.md`; a Phase 1 filter must reproduce that rule's output for
  every one of the 20 queries, not just the one driven here.
- **Filter latency.** `view.filter_samples` counts per-keystroke re-filters since the
  browser was last opened (0 right after opening, proving it is per-open, not
  cumulative); `view.filter_p99_ms` must read under 16 ms after the 50-keystroke
  script.
- **Side effect.** The `opened` sink stays `[]`.

## Gotchas

- Read `metrics` for the first-frame timing **before** any `see`/`a11y` call on that
  instance — activating the AccessKit tree makes GPUI build it on every frame for the
  window's life, which would slow the very frame being timed.
- The filter ranks on **both** `name` and `uri`, case-folded, NOT NFKC-normalised
  (these are identifiers, not prose) — a query that only matches one field of an item
  still ranks it; an item matching neither field is dropped entirely, never shown at
  a low rank. `probes/fixtures/README.md`'s worked examples (`gthb`, `rddt`, the
  `ab`/`aabb` minimum-window case) are the edge cases a naive "contains" or greedy
  subsequence scorer gets wrong.
- The filter `TextInput` keeps focus while arrows/Home/End/Enter/Right/Left drive the
  list (A11Y-NOTES.md's "Navigating a list while an Input keeps focus"): Up/Down/
  Home/End are swallowed by the Input unless the browser intercepts them on a parent
  with a capture-phase action listener — a filter that "looks" keyboard-driven in
  isolation can still fail list navigation once it is the one holding focus.
- Today's stub shows only its static title and has no `TextInput` or `ListItem`
  nodes at all, so this call fails at the first `see` until Phase 1 lands.
