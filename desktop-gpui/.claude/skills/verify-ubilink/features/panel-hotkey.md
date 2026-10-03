# panel-hotkey: Hotkey-to-first-frame stays under budget with real views

- id: panel-hotkey
- proven: no
- anchors: src/shell/panel.rs:PanelView, src/shell/panel.rs:arm_first_frame, src/shell/hotkeys.rs:bindings, src/automation/store.rs:MetricKind, probes/panel-hotkey.json
- constraints: linking-manifesto-app:U1

## What it is

From the moment a hotkey is pressed to the moment the panel's first frame actually
presents, p99 stays under 200 ms and p50 under 80 ms, over 50 triggers — this held
at Phase 0 against the stub views (`shell.md`'s proof), and Phase 1 R15 re-asserts
it now that the panel has real content to lay out and paint, not just a static
title.

## How to reach it

- **The three hotkeys**, cycling capture/browser/settings
  (`trigger_hotkey {"action": ...}`), toggled hidden between each trigger so every
  show is a fresh window.
- **Not driven here:** opening through the tray's `open_view` — this feature times
  only the hotkey path, the one real Carbon events and `trigger_hotkey` both enter
  through `shell::handle_event`.

## Drive it

The acceptance proof is the frozen scenario `probes/panel-hotkey.json`, run through
the harness's `scenario` command. It drives all 50 triggers and checks the real
p50/p99, not a handful of samples.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name panel-hotkey
echo "exit $?"
```

Expected: exit 0. `$OUT/panel-hotkey/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows 50 "trigger N/50 opens" / "trigger N/50 timed" / "trigger
N/50 hides" triplets, then PASS for "U1 budget over 50 triggers", "placement: real
popup", "within the cursor's display", "within the display state names" and
"within the main display".

## Proof

- **p99 < 200 ms, p50 < 80 ms over 50 triggers.** "U1 budget over 50 triggers"
  requires `summary.hotkey_to_first_frame.count >= 50`,
  `summary.hotkey_to_first_frame.p50 < 80` and `.p99 < 200` (nearest-rank
  percentiles, so p99 of 50 samples is the single slowest one). Each trigger is its
  own block, so a focus-theft retry may add a sample but the count check uses
  `ge`, never an exact count.
- **t0/t1 are the real boundary.** t0 is `shell::handle_event` receiving the event
  — the same entry point a real Carbon hotkey uses; t1 is after the first platform
  frame's present returned (`PanelView`'s `arm_first_frame`), not merely after the
  state update.
- **Placement.** "placement: real popup" through "within the main display" prove
  the panel opens within the display under the cursor and within the display
  `state.shell.display_id` names.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/panel-hotkey"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature panel-hotkey \
  --verifier "$VERIFIER" --result pass \
  --action "scenario panel-hotkey (desktop-gpui/probes/panel-hotkey.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "no side effect asserted beyond timing and placement; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- This is a *regression* proof carried over from Phase 0 (spec T4): the budget
  itself does not change in Phase 1, only the content being measured does. A Phase
  1 view that is heavier to lay out or paint can regress this number even though
  nothing about the hotkey path itself changed.
- `probes/performance.json` embeds an identical copy of this exact 50-trigger loop
  (its own notes say so: "Copied here at Phase 0 because Phase 1 may change
  neither file"), as part of writing `docs/PERFORMANCE.md`'s `hotkey_p50_ms` /
  `hotkey_p99_ms` rows — that run (`resource-budget.md`'s scenario) is what
  actually produces R15's written record for this number; this feature's own
  `panel-hotkey` scenario is the narrower, faster regression check that must also
  hold on its own, and is what this feature's evidence record pins.
- Every trigger is a focus-theft-retryable block (`probes/README.md`'s Unattended
  guards): a second theft during the same trigger is ENV-BLOCKED, not a product
  FAIL.
- `probes/panel-hotkey.json` and `probes/panel-multidisplay.json` are protected
  Phase 0 baseline files (spec T4): Phase 1 may not edit them, and neither may this
  skill's own harness scripts — only this feature file, under `features/`, changes.
