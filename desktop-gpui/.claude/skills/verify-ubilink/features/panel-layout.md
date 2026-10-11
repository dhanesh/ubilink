# panel-layout: The panel fits its content; Settings' buttons and texts match

- id: panel-layout
- proven: no
- anchors: src/views/layout.rs, probes/settings-fit.json, probes/settings-buttons.json, probes/settings-login-text.json, probes/capture-fit.json

## What it is

A hands-on check of the shipped app found three parity regressions that the
accessibility tree cannot show (Phase 2b regressions 2-4):

- **Height.** The panel was a fixed 640x140 for every route. Settings' adapter grid
  was clipped after its first row, and capture's third backlink row was cut off.
  Settings now opens at its content height, and capture grows to its backlinks when
  the capture lands. Both refit in place, within 96..900 and the display's usable
  area. The Tauri reference is `resize_panel` and `fitWindow`.
- **Buttons.** Settings' Export was a large primary button while Import and Reset…
  were small. All action buttons are now 28 pt tall (Tauri `.set-btn`).
- **Login text.** The launch-at-login status was larger than its label. Both are now
  12 pt (`font_size::META`).

## How to reach it

- `trigger_hotkey settings`. The reset question is opened with Tab to `Reset…`, then
  Enter, and closed with Escape. The login switch is toggled with Tab to it, then
  Space.
- `trigger_hotkey capture`, with `graph-backlinks.json` seeded and the
  `capture-subject.json` capture fixture (3 backlinks). `graph-backlinks-newer.json`
  is then merged for 4 backlinks.
- **What is measured.** The views report measured layout in `state.view.layout`
  (`views/layout.rs`): bounds, the viewport, and resolved text sizes. Steps compare
  two of those values with `{"path": ...}` operands (probes/README.md, Expectations).

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
SCENARIOS="settings-fit settings-buttons settings-login-text capture-fit"
for name in $SCENARIOS; do
  python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name "$name"
  echo "$name exit $?"
done
```

Expected: every scenario exits 0 and reports `verdict` `"PASS"`.

## Proof

- **settings-fit.** The last adapter switch ends inside the grid's scroll box, and
  the box ends inside the window. This holds when shown, while the reset question is
  open, and after it closes. `winframe: state` keeps the window on its display.
- **settings-buttons.** Import, Reset…, Delete everything and Cancel each have
  Export's measured height.
- **settings-login-text.** The status (`Not registered`, then `Enabled`) has the
  label's resolved text size, which is 12.
- **capture-fit.** With 3 and with 4 backlinks, the last row ends inside the list's
  scroll box, and the box ends inside the window.

Record one evidence set for all four scenarios, as `browser-navigation.md` does:

```sh
python3 "$H" doctor --instance "$INSTANCE"
mkdir -p "$WT/.verify-run/$INSTANCE"
REC="$(mktemp -d "$WT/.verify-run/$INSTANCE/record-panel-layout.XXXXXX")"
RESULT=pass
for name in $SCENARIOS; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature panel-layout \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario(s) $SCENARIOS (desktop-gpui/probes/<name>.json) on instance $INSTANCE" \
  --observed "$(for name in $SCENARIOS; do printf '%s: %s; ' "$name" "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/$name/result.json")"; done)" \
  --side-effect "settings-login-text registers through the login_item fixture copy only; nothing opened; see each scenario's renamed result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- **Measured, not declared.** A value is whatever the layout produced that frame. An
  element that is not drawn reads as missing, and a step needing it fails.
- **Refits are async.** A refit (the reset question, a capture landing) sets the
  window frame from a foreground task, so read layout with `retry_ms`.
- **The settings height depends on login state.** It includes the login row's second
  line whenever there is an explanation or an error. Without a `login_item` fixture
  that line is an error, so the panel is 14 pt taller.
- **Not provable here.** Settings' Import dialog in front of the panel, and the
  Finder reveal after Export, are hand checks (docs/RUNBOOK-gui-panel.md, O4).
