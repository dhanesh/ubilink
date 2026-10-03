# link-marking: Cmd-L marks two links and connects them both ways

- id: link-marking
- proven: no
- anchors: probes/link-marking.json, probes/fixtures/graph-backlinks.json
- constraints: linking-manifesto-app:T2

## What it is

Pressing Cmd-L on one link, moving to a second, and pressing Cmd-L again links them
both ways in the graph and shows a confirmation naming both. Phase 1 R8, matching
the Tauri browser's own Cmd-L binding (plain `L` still types into the always-focused
filter — spec ruling R-n). After a real restart, drilling into the second link must
still list the first among its connections.

## How to reach it

- **Cmd-L** (`key {"keys": "cmd-l"}`) on the selected row, twice (select A, mark,
  select B, mark). Cmd-L is not a bound action the filter `Input` consumes — it has
  no simulated `key_char` (a platform modifier), so it reaches the browser's own
  `on_key_down` listener directly (A11Y-NOTES.md).
- **Not driven here:** typing `l` (lowercase, unmodified) into the filter, which is
  `link-browser.md`'s territory, not a mark action.

## Drive it

The acceptance proof is the frozen scenario `probes/link-marking.json`, run with the
harness's `scenario` command. The scenario marks A then B, confirms the graph holds
the bidirectional link, restarts the instance in place (probe.py's own `restart`
step, run by `scenario` itself — SKILL.md's "What `scenario` does"), reopens the
browser, and drills into B to confirm A is listed among its connections after the
restart.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name link-marking
echo "exit $?"
```

Expected: exit 0. `$OUT/link-marking/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "an a11y node confirms the link", "the graph links A
and B (one unordered row: both directions)", the `restart` line, "drilled into B"
and "B's connections list A (and its existing link)".

## Proof

- **Confirmation.** "an a11y node confirms the link" requires a `Label` reading
  `Linked "<A name>" ↔ "<B name>"` after the second Cmd-L (README's exact Tauri
  wording).
- **Bidirectional in the graph.** "the graph links A and B (one unordered row: both
  directions)" reads the instance's own `ubilink-graph.sqlite` directly
  (`links: [[A, B]]`): ubilink-core stores one row per unordered pair and
  `links_of` reads either end, so this one check is the proof that both
  `links_of(A) ∋ B` and `links_of(B) ∋ A` hold, not just that *some* row was
  written.
- **Survives a real restart.** The scenario's `restart` step relaunches the same
  binary with the same data dir kept; "drilled into B" and "B's connections list A
  (and its existing link)" after it prove the link lives in the graph the view
  reads on every open, not only in whatever state the view held before the
  restart.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.

Record it (SKILL.md, Evidence) — the scenario's own restart step is captured inside
`$OUT/link-marking/` as one of its per-step artifacts, so no separate restart record
is needed:

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/link-marking"
RESULT=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print("pass" if r["exit"]==0 else "fail")' "$R/result.json")
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature link-marking \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario link-marking (desktop-gpui/probes/link-marking.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "opened sink stayed []; the graph holds the bidirectional link before and after a real restart; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `scenario`'s `restart` step is probe.py's own, run inside the scenario — it is
  not the harness's separate `restart` command (SKILL.md's "Proving persistence"
  section is for hand-written `call` sequences only); a verifier never issues
  `restart` by hand for this feature.
- Plain `l` (no Cmd) must keep typing into the filter — a browser that intercepts
  bare `l` as a mark shortcut breaks `link-browser.md`'s filter contract for every
  query containing the letter.
- Today's stub has no `ListItem` rows and no Cmd-L handling at all, so `scenario
  --name link-marking` exits 1 at its first `see` until Phase 1 lands.
