# panel-dismiss: Capture and browser hide on loss of key status; settings and onboarding don't

- id: panel-dismiss
- proven: no
- anchors: src/shell/panel.rs:PanelView, src/shell/panel.rs:activation_changed, probes/panel-dismiss.json

## What it is

Capture and browser hide the instant the panel loses key status — a click-away or
any other app becoming active — because they are glanceable actions, not places a
user lingers. Settings and onboarding stay open through the same loss of key
status, because a user may switch apps from them on purpose (to grant a permission,
say) without meaning to dismiss the panel. `shell.md` already proves this against
the Phase 0 stubs; Phase 1 R15 re-asserts it now that each route has real content
and real controls that could, in principle, change what deactivation does.

## How to reach it

- **`resign_key`**, the click-away observer's own code path.
- **A real loss of key status** (`activate_other`), the probe's own activator app
  taking focus and quitting, so the deactivation is real AppKit behaviour, not
  simulated.
- Onboarding has no hotkey; the scenario reaches it with `open_view
  {"route": "onboarding"}`.

## Drive it

The acceptance proof is the frozen scenario `probes/panel-dismiss.json`, run
through the harness's `scenario` command. It drives all four routes
(capture, browser, settings, onboarding) × both dismiss paths (`resign_key`, a real
`activate_other`), not a subset.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name panel-dismiss
echo "exit $?"
```

Expected: exit 0. `$OUT/panel-dismiss/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows, for each of `capture` and `browser`, PASS for "...: resign_key
hides", "...: shell has no panel", "...: window left the screen (resign_key)",
"...: real loss of key status", "...: hidden by the real deactivation" and "...:
window left the screen (activate_other)"; for each of `settings` and `onboarding`,
PASS for "...: resign_key keeps it", "...: still on screen (resign_key)", "...:
real loss of key status", "...: still shown after the real deactivation" and
"...: still on screen (activate_other)".

## Proof

- **Capture and browser hide.** Both `resign_key` (the click-away observer's code)
  and a real `activate_other` deactivation close the panel: `state.shell.visible`
  false and the window gone from Quartz's on-screen list, for both routes.
- **Settings and onboarding stay open.** Neither `resign_key` nor a real
  deactivation hides them: `state.shell.visible` stays true on the same route, and
  the window is still on screen, for both routes.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/panel-dismiss"
RESULT=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print("pass" if r["exit"]==0 else "fail")' "$R/result.json")
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature panel-dismiss \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario panel-dismiss (desktop-gpui/probes/panel-dismiss.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "opened sink stayed []; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `probes/panel-dismiss.json` is a protected Phase 0 baseline file (spec T4): Phase
  1 may not edit it, and neither may this skill's harness — only this feature file
  changes for this fix round.
- `probes/performance.json` embeds an identical copy of these same
  resign_key/activate_other cases per route (its own notes: copied here at Phase 0
  because Phase 1 may change neither file), as part of the `resource-budget.md`
  scenario's own run; this feature's own `panel-dismiss` scenario is the narrower
  regression check whose own evidence record this file pins.
- A focus theft during a visibility check re-runs that case's block once from a
  closed panel; a second theft is ENV-BLOCKED, never a product FAIL.
- `activate_other` briefly takes real key status from whatever is in front on this
  desktop, and holds the session lease/desktop lock for the duration — it cannot
  run concurrently with anyone else's drive.
