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
- **Tab to a step's button, then Enter** (`step:{"key_until": {...}, "key": "tab"}`,
  then `key {"keys": "enter"}`) to open its pane.
- **`R`** (`key {"keys": "r"}`) to re-check permission state.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/onboarding"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'load_fixture:{"kind":"permission","path":"{fixtures}/permissions-two-missing.json"}' \
  'open_view:{"route":"onboarding"}' \
  'wait_state:{"shell.visible":true,"route":"onboarding"}' \
  'step:{"call":"window_id","save":{"wid":"window_id"}}' \
  'see:{"role":"ListItem","count":2}' \
  'see:{"text":"Accessibility"}' \
  'see:{"text":"Automation"}' \
  'see:{"role":"ListItem","text":"Not granted","deep":true,"count":2}' \
  opened \
  'step:{"key_until":{"role":"Button","text":"Automation","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"enter"}' \
  'step:{"call":"opened","retry_ms":3000,"expect":{"":{"min_len":1}}}' \
  'load_fixture:{"kind":"permission","path":"{fixtures}/permissions-all-granted.json"}' \
  'key:{"keys":"r"}' \
  'see:{"role":"ListItem","text":"Granted","exact":true,"deep":true,"count":2}' \
  'absent:{"text":"Not granted"}' \
  'step:{"call":"window_id","expect":{"window_id":{"saved":"wid"}}}' \
  'step:{"process":"unchanged"}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. Before any button is activated,
`opened` reports `"result": []`; after activating Automation's button it grows by
one entry whose target is `ubilink-app`'s `settings_pane_url("automation")` exactly
(the Phase 1 scenario, `probes/onboarding.json`, asserts that URL verbatim).

## Proof

- **Two steps, named and unmet.** Exactly two `ListItem` rows exist, one per missing
  permission, each naming it and reading `Not granted`.
- **The button opens the right pane.** Activating the Automation step's button
  (reached by `key_until`, so the proof never hard-codes a keyboard layout) makes the
  `opened` sink's newest entry match that permission's exact System Settings pane
  URL — through the opener, never a real `open`.
- **Re-check, same window.** After the fixture flips to all-granted and `R` is
  pressed, the row shows `Granted` (exact text) with no `Not granted` left anywhere,
  and the window id and process are unchanged — proving Re-check updates state in
  place rather than reopening.
- **Side effect.** `opened` only ever grows from the button press, never from `R`.

## Gotchas

- "Granted" is matched exactly (`exact: true` in the real Phase 1 scenario) so a
  residual "Not granted" row cannot satisfy it by substring.
- The pane URL comes from `ubilink-app`'s own `settings_pane_url`, not from the
  fixture — the fixture only says which permissions are missing, never which pane to
  open.
- `key_until` proves the button is reachable from the keyboard without this map
  committing to a specific keyboard focus sequence for the view — a later reordering
  of the steps does not break this proof as long as the button is still reachable.
- Today's stub shows only its static title with no permission rows at all, so this
  call fails at the first `see` until Phase 1 lands.
