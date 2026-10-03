# theming: Every view paints in light and dark token colours, live

- id: theming
- proven: no
- anchors: src/views/capture.rs, src/views/browser.rs, src/views/onboarding.rs, src/views/settings.rs, src/theme/mod.rs:sync, src/theme/convert.rs:light_theme_config, src/theme/convert.rs:dark_theme_config, probes/theming.json

## What it is

Every view draws its surfaces, text and accent control in the token-derived light
and dark colours, and switches between them the moment the system appearance
changes — no new window, no restart. Phase 1 R14.

## How to reach it

- **`set_appearance`** (`set_appearance {"mode": "light"|"dark"}`) on an open view,
  the automation seam that stands in for a real system appearance change.
- **Not driven here:** a real macOS appearance toggle — automation builds only have
  the `set_appearance` verb; this is the production boundary the verb substitutes
  for (probes/README.md).

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/theming"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'load_fixture:{"kind":"capture","path":"{fixtures}/capture-ok.json"}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture"}' \
  'step:{"call":"window_id","save":{"wid":"window_id"}}' \
  'set_appearance:{"mode":"light"}' \
  'step:{"process":"unchanged"}' \
  'step:{"call":"window_id","expect":{"window_id":{"saved":"wid"}}}' \
  'step:{"pixel":"view.theme_samples.surface","color":"#fafafc","delta_e":3}' \
  'step:{"pixel":"view.theme_samples.text","color":"#1e1e1e","delta_e":3,"match":"best"}' \
  'step:{"pixel":"view.theme_samples.accent","color":"#007aff","delta_e":3}' \
  screenshot:capture-light \
  'set_appearance:{"mode":"dark"}' \
  'step:{"process":"unchanged"}' \
  'step:{"call":"window_id","expect":{"window_id":{"saved":"wid"}}}' \
  'step:{"pixel":"view.theme_samples.surface","color":"#1c1c1e","delta_e":3}' \
  'step:{"pixel":"view.theme_samples.text","color":"#f6f6f6","delta_e":3,"match":"best"}' \
  'step:{"pixel":"view.theme_samples.accent","color":"#0a84ff","delta_e":3}' \
  screenshot:capture-dark \
  'set_appearance:{"mode":"system"}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0; two PNGs land in `$OUT`. Repeat
the same shape for `browser`, `onboarding` and `settings` (opened with
`open_view`/`trigger_hotkey` as each route requires) for the full proof; the Phase 1
scenario (`probes/theming.json`) drives all 4 views × 2 appearances × 3 samples, 24
pixel checks and 8 screenshots in one run, restoring the system appearance at the
end.

## Proof

- **Same window, same process.** `set_appearance` changes colours without a new
  window id or a different process.
- **Within tolerance of the token colours.** Each of the three samples —
  an opaque surface, a run of primary text (best-matching pixel, since antialiased
  edges are not the text colour), and the accent control — is within ΔE 3 of the
  light/dark values this map states explicitly (not read from `design/` at proof
  time, so the proof cannot drift silently with the tokens).
- **Real regions, not swatches.** Each view reports its own sample regions in
  `view.theme_samples.{surface,text,accent}`; a region under 24×24 pt fails
  (`STATE_REGION_MIN_PT`), so a view cannot point the probe at a token-coloured
  rectangle painted only for this check.
- **Screenshots recorded** for both appearances of every view, as artifacts for the
  evidence record.

## Gotchas

- Pixel and screenshot steps need Screen Recording for the process running the
  harness; without the grant they are ENV-BLOCKED (exit 3), never a FAIL — drop
  those steps and say so in `--observed` if the grant is missing on this host.
- `uiBg` and `text1` are translucent and the panel itself is blurred, so the expected
  colours above are the tokens' RGB painted at full opacity (surface) and
  alpha-composited over that surface (text), not the raw token values — see
  `probes/theming.json`'s notes for the exact composite math.
- Restore the system appearance (`set_appearance: {"mode": "system"}`) at the end of
  the call, as shown above, so the host is not left pinned to one appearance.
- Today's stub paints only its title in the theme's foreground colour and reports no
  `view.theme_samples` region at all, so the `pixel` steps fail (a state path with no
  region is a FAIL) until Phase 1 lands.
