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

- **`resign_key`** (`step:{"call": "resign_key", "expect": {...}}`), the click-away
  observer's own code path.
- **A real loss of key status** (`step:{"activate_other": true}`), the probe's own
  activator app taking focus and quitting, so the deactivation is real AppKit
  behaviour, not simulated.
- Onboarding has no hotkey; reach it with `open_view {"route": "onboarding"}`.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/panel-dismiss"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture"}' \
  'step:{"winframe":"any"}' \
  'step:{"call":"resign_key","expect":{"route":"capture","closed":true,"state.shell.visible":false}}' \
  'step:{"winframe":"gone"}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture"}' \
  'step:{"activate_other":true}' \
  'wait_state:{"shell.visible":false}' \
  'step:{"winframe":"gone"}' \
  'open_view:{"route":"settings"}' \
  'wait_state:{"shell.visible":true,"route":"settings"}' \
  'step:{"call":"resign_key","expect":{"route":"settings","closed":false,"state.shell.visible":true}}' \
  'step:{"activate_other":true,"settle_ms":800}' \
  'step:{"call":"state","expect":{"shell.visible":true,"route":"settings"}}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. The full Phase 1 regression
(`probes/panel-dismiss.json`) repeats this for `browser` the same way as `capture`,
and for `onboarding` the same way as `settings`, each case its own focus-theft-
retryable block.

## Proof

- **Capture and browser hide.** Both `resign_key` (the click-away observer's code)
  and a real `activate_other` deactivation close the panel: `state.shell.visible`
  false and the window gone from Quartz's on-screen list.
- **Settings and onboarding stay open.** Neither `resign_key` nor a real
  deactivation hides them: `state.shell.visible` stays true on the same route, and
  the window is still on screen.

## Gotchas

- `probes/panel-dismiss.json` is a protected Phase 0 baseline file (spec T4): Phase
  1 may not edit it, and neither may this skill's harness — only this feature file
  changes for Task 15.
- A focus theft during a visibility check re-runs that case's block once from a
  closed panel; a second theft is ENV-BLOCKED, never a product FAIL.
- `activate_other` briefly takes real key status from whatever is in front on this
  desktop, and holds the session lease/desktop lock for the duration — it cannot
  run concurrently with anyone else's drive.
