# settings-permissions: Settings shows Automation's status and opens permission setup; capture goes to setup until Automation is granted

- id: settings-permissions
- proven: no
- anchors: probes/settings-permissions.json, probes/capture-permission-gate.json, probes/fixtures/permissions-two-missing.json, probes/fixtures/permissions-all-granted.json
- constraints: linking-manifesto-app:S4

## What it is

rc.4 parity with the Tauri app's Settings › Permissions and its "onboarding until
Automation is granted":

- Settings' first section, **Permissions**, has an Automation row ("Read the active
  app's link.") with a status Label, exactly `Allowed`, `Not allowed` or `Not checked
  yet`, from the same check onboarding uses, run off the main thread each time
  Settings opens. Its **Open permission setup…** button switches the same panel to
  onboarding.
- Opening capture (⌘⇧K, the tray's Capture, a reopen) while Automation is not
  confirmed this session checks the permission report first: not granted goes to
  onboarding and nothing is captured. A report (or the release start-up probe) that
  says granted confirms it for the session: captures then never check again, and
  onboarding does not come back.

## How to reach it

- **⌘⇧,** (`trigger_hotkey {"action": "settings"}`), then Tab to `Open permission
  setup…` and Enter.
- **⌘⇧K** (`trigger_hotkey {"action": "capture"}`) or the tray's Capture
  (`open_view {"route": "capture"}`) with a `permission` fixture loaded.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
for name in settings-permissions capture-permission-gate; do
  python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name "$name"
  echo "$name exit $?"
done
```

Expected: both exit 0 with `verdict` `"PASS"` in `$OUT/<name>/result.json`.

## Proof

- **Status, both ways.** settings-permissions' "the Automation status reads Not
  allowed" (two-missing fixture) and "the Automation status reads Allowed, exactly"
  (all-granted, `exact: true`, plus "no Not allowed").
- **Section order.** "the Permissions section sits above App adapters" is an ordered
  Label sequence.
- **Button routes in place.** "the button routes to onboarding", "same window id",
  "same process", and "nothing opened" (it opens onboarding, not System Settings).
- **Capture gate.** capture-permission-gate's "capture open routes to onboarding"
  for the hotkey and the tray, each with "nothing was captured" (graph
  `resource_count` 0); with Automation granted "still on capture, phase captured";
  and after that, with the report flipped back to not granted, "onboarding does not
  reappear".

Record it as in SKILL.md's Evidence section, with `--feature settings-permissions`
and one `--artifact` per file in each scenario's result directory.

## Gotchas

- `Allowed` is a substring of `Not allowed`: match it with `exact: true`.
- Restarts replay the setup fixtures, so a round that needs another report loads it
  after the restart.
- capture-permission-route.json covers the older failed-capture route; this gate uses
  a succeeding capture fixture, so only the permission report can move the panel.
