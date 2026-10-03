# onboarding: Missing permissions get a step each, with a pane and a re-check

- id: onboarding
- proven: no
- anchors: src/views/onboarding.rs:OnboardingView, probes/onboarding.json, probes/fixtures/permissions-two-missing.json, probes/fixtures/permissions-all-granted.json
- constraints: linking-manifesto-app:S4

## What it is

Onboarding shows one step per missing permission (Accessibility, Automation), each
naming the permission and whether it is granted. Activating a step's button opens
that exact permission's System Settings pane; Re-check re-reads permission state in
the same window, without reopening it. Phase 1 R9; spec S4 ("least-privilege OS
automation with transparent rationale" — the view explains *why* each permission is
asked for and *where* to grant it, rather than failing silently).

## How to reach it

- **The tray's Open on `onboarding`** (`open_view {"route": "onboarding"}`) — there
  is no hotkey for this route (`shell/mod.rs`'s `Route` doc comment).
- **Tab to a step's button, then Enter** (the scenario's `key_until` step, then
  `key {"keys": "enter"}`) to open its pane.
- **`R`** (`key {"keys": "r"}`) to re-check permission state.

## Drive it

The acceptance proof is the frozen scenario `probes/onboarding.json`, run through
the harness's `scenario` command.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name onboarding
echo "exit $?"
```

Expected: exit 0. `$OUT/onboarding/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "two steps", "one per missing permission", "both
not granted", "the opened sink reports the Automation pane URL exactly", "Automation
now shows granted", "both steps granted, none left 'Not granted'", "same window id"
and "same process".

## Proof

- **Two steps, named and unmet.** "two steps" and "both not granted" require
  exactly two `ListItem` rows, each reading `Not granted`.
- **The button opens the right pane.** "Tab to the Automation step's button"
  (`key_until`, so the proof never hard-codes a keyboard layout) then "the opened
  sink reports the Automation pane URL exactly" requires the sink's newest entry to
  match `ubilink-app`'s `settings_pane_url("automation")` exactly — through the
  opener, never a real `open`.
- **Re-check, same window.** After `load_fixture permissions-all-granted.json` and
  `key r`, "Automation now shows granted" and "both steps granted, none left 'Not
  granted'" require the exact text `Granted`; "same window id" compares the saved
  `window_id` from before any button was pressed against the one read after R; "same
  process" requires the pid to be unchanged — together proving Re-check updates
  state in place rather than reopening.
- **Side effect.** "nothing opened yet" / "Re-check opened nothing more" bracket the
  one expected growth of the `opened` sink (the button press), so `R` itself never
  opens anything.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/onboarding"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature onboarding \
  --verifier "$VERIFIER" --result pass \
  --action "scenario onboarding (desktop-gpui/probes/onboarding.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "opened sink grows only from the button press, never from R; same window id and pid across Re-check; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- "Granted" is matched exactly (`exact: true`) so a residual "Not granted" row
  cannot satisfy it by substring.
- The pane URL comes from `ubilink-app`'s own `settings_pane_url`, not from the
  fixture — the fixture only says which permissions are missing, never which pane to
  open.
- `key_until` proves the button is reachable from the keyboard without the scenario
  committing to a specific keyboard focus sequence for the view — a later
  reordering of the view's layout does not break this proof as long as the button
  is still reachable.
- Today's stub shows only its static title with no permission rows at all, so
  `scenario --name onboarding` exits 1 at the first `see` until Phase 1 lands.
