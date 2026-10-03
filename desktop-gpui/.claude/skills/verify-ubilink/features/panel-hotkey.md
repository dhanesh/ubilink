# panel-hotkey: Hotkey-to-first-frame stays under budget with real views

- id: panel-hotkey
- proven: no
- anchors: src/shell/panel.rs:PanelView, src/shell/panel.rs:arm_first_frame, src/shell/hotkeys.rs:bindings, src/automation/store.rs:MetricKind, probes/panel-hotkey.json
- constraints: linking-manifesto-app:U1

## What it is

From the moment a hotkey is pressed to the moment the panel's first frame actually
presents, p99 stays under 200 ms and p50 under 80 ms, over 50 triggers — this held
at Phase 0 against the stub views (`shell.md`'s proof), and Phase 1 R15 re-asserts it
now that the panel has real content to lay out and paint, not just a static title.

## How to reach it

- **The three hotkeys**, cycling capture/browser/settings
  (`trigger_hotkey {"action": ...}`), toggled hidden between each trigger so every
  show is a fresh window.
- **Not driven here:** opening through the tray's `open_view` — this feature times
  only the hotkey path, the one real Carbon events and `trigger_hotkey` both enter
  through `shell::handle_event`.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/panel-hotkey"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'step:{"call":"metrics","expect":{"summary.hotkey_to_first_frame.count":0}}' \
  'step:{"warp_cursor":0}' \
  'trigger_hotkey:{"action":"capture"}' \
  'step:{"call":"metrics","retry_ms":3000,"expect":{"summary.hotkey_to_first_frame.count":{"ge":1}}}' \
  'trigger_hotkey:{"action":"capture"}' \
  'trigger_hotkey:{"action":"browser"}' \
  'step:{"call":"metrics","retry_ms":3000,"expect":{"summary.hotkey_to_first_frame.count":{"ge":2}}}' \
  'trigger_hotkey:{"action":"browser"}' \
  'trigger_hotkey:{"action":"settings"}' \
  'step:{"call":"metrics","retry_ms":3000,"expect":{"summary.hotkey_to_first_frame.count":{"ge":3}}}' \
  'trigger_hotkey:{"action":"settings"}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. The three triggers above stand in
for the Phase 1 scenario's full 50-trigger loop
(`probes/panel-hotkey.json`, cycling the three routes and toggling hidden between
each); its final assertion is
`summary.hotkey_to_first_frame.p99 < 200` and `.p50 < 80` over all 50 samples. No
`see`/`a11y` call happens before or during the loop in either version — activating a
window's AccessKit tree makes GPUI build it on every frame for that window's life,
which would slow the very thing being timed.

## Proof

- **p99 < 200 ms, p50 < 80 ms over 50 triggers.** `summary.hotkey_to_first_frame`
  uses nearest-rank percentiles (so p99 of 50 samples is the single slowest one);
  each trigger is its own block, so a focus-theft retry may add a sample but the
  count check uses `ge`, never an exact count.
- **t0/t1 are the real boundary.** t0 is `shell::handle_event` receiving the event —
  the same entry point a real Carbon hotkey uses; t1 is after the first platform
  frame's present returned (`PanelView`'s `arm_first_frame`), not merely after the
  state update.
- **Placement** (not reproduced in the short command above): the panel opens within
  the display under the cursor and within the display `state.shell.display_id`
  names (`probes/panel-hotkey.json`'s placement steps).

## Gotchas

- This is a *regression* proof carried over from Phase 0 (`shell.md` covers the
  stub's own latency implicitly through `panel-hotkey.json`/`panel-dismiss.json`
  being Phase 0 artifacts, spec T4): the budget itself does not change in Phase 1,
  only the content being measured does. A Phase 1 view that is heavier to lay out or
  paint can regress this number even though nothing about the hotkey path itself
  changed.
- Every trigger is a focus-theft-retryable block (`probes/README.md`'s Unattended
  guards): a second theft during the same trigger is ENV-BLOCKED, not a product
  FAIL.
- `probes/panel-hotkey.json` and `probes/panel-multidisplay.json` are protected
  Phase 0 baseline files (spec T4): Phase 1 may not edit them, and neither may this
  skill's own harness scripts — only this feature file, under `features/`, changes.
