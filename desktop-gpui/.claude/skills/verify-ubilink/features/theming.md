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

The acceptance proof is the frozen scenario `probes/theming.json`, run through the
harness's `scenario` command. It drives all 4 views × 2 appearances × 3 samples (24
pixel checks) and records 8 screenshots in one run — not one view sampled as a
stand-in for the rest.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name theming
echo "exit $?"
```

Expected: exit 0. `$OUT/theming/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows, for each of `capture`, `browser`, `onboarding`, `settings` ×
`light`, `dark`, PASS lines for "...: same process", "...: opaque surface", "...:
primary text run", "...: accent control" and "...: screenshot recorded" — 8
screenshot PNGs land under `$OUT/theming/`.

## Proof

- **Same window, same process, for every view.** Each view/appearance pair's
  "...: same process" and "...: same window, still on ..." steps require
  `set_appearance` to change colours without a new window id or a different
  process, for all 4 views.
- **Within tolerance of the token colours, all 24 samples.** Each view/appearance
  pair's three pixel steps (surface, text, accent) are each within ΔE 3 of the
  scenario's own stated colours — light surface `#fafafc`, text `#1e1e1e`, accent
  `#007aff`; dark surface `#1c1c1e`, text `#f6f6f6`, accent `#0a84ff` — read from
  `view.theme_samples`, not from `design/` at proof time, so the proof cannot drift
  silently with the tokens.
- **Real regions, not swatches.** Each view reports its own sample regions in
  `view.theme_samples.{surface,text,accent}`; a region under 24×24 pt fails
  (`STATE_REGION_MIN_PT`), so a view cannot point the probe at a token-coloured
  rectangle painted only for this check.
- **All 8 screenshots recorded**, as artifacts in the evidence record.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/theming"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature theming \
  --verifier "$VERIFIER" --result pass \
  --action "scenario theming (desktop-gpui/probes/theming.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "same pid/window id across every appearance switch, for all 4 views; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- Pixel and screenshot steps need Screen Recording for the process running the
  harness; without the grant they are ENV-BLOCKED (exit 3), never a FAIL — the
  scenario's own report distinguishes this from a product FAIL, and `--observed`
  should say so if the grant is missing on this host.
- `uiBg` and `text1` are translucent and the panel itself is blurred, so the
  expected colours above are the tokens' RGB painted at full opacity (surface) and
  alpha-composited over that surface (text), not the raw token values — see
  `probes/theming.json`'s notes for the exact composite math.
- The scenario restores the system appearance (`set_appearance: {"mode": "system"}`)
  at the end of its own run.
- Today's stub paints only its title in the theme's foreground colour and reports
  no `view.theme_samples` region at all, so `scenario --name theming` exits 1 at its
  first `pixel` step (a state path with no region is a FAIL) until Phase 1 lands.
