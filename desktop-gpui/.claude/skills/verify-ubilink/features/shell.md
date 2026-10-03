# shell: Menu-bar app and its pop-up panel

- id: shell
- proven: no
- anchors: src/shell/mod.rs:handle_event, src/shell/panel.rs:PanelView, src/shell/state.rs:ShellState, src/shell/tray.rs:command_for, src/shell/hotkeys.rs:bindings, src/automation/mod.rs:state_json, src/paths.rs, probes/shell.json

## What it is

ubilink lives in the menu bar, not the Dock. Pressing a global hotkey (or choosing an
item in the tray menu) pops up one translucent, blurred panel near the cursor, showing
the view that hotkey belongs to: Capture, Browse links or Settings. Switching views
reuses the same panel; nothing else opens or launches. Every view built on top of the
shell (capture, browser, settings, onboarding) depends on this working first.

## How to reach it

- Global hotkeys (`src/shell/hotkeys.rs`, `bindings`): Cmd+Shift+K opens capture,
  Cmd+Shift+L opens the link browser, Cmd+Shift+, opens settings. Automation instances
  skip registering them (`UBILINK_SKIP_HOTKEYS=1`), so the verifier drives the same code
  path with `trigger_hotkey {"action": "capture" | "browser" | "settings"}`, which posts
  the event into `shell::handle_event` exactly where a real Carbon hotkey event enters.
- Tray menu items `capture`, `browse`, `settings`, `quit` (`src/shell/tray.rs`,
  `command_for`) map to `Command::Open(route)`, the same command the `open_view` verb
  executes (`open_view {"route": "browser"}`).
- The three real hotkey registrations themselves are a maintainer's attended check
  (probes/shell.json notes): an automation instance must never grab the user's keys.

## Drive it

On a launched instance, after a passing doctor (SKILL.md, Launch and Doctor), from the
repository root with the session variables set:

```sh
OUT="$(git rev-parse --show-toplevel)/.verify-run/$INSTANCE/captures/shell"
python3 "$H" call --instance "$INSTANCE" --out "$OUT" \
  'wait_state:{"shell.tray_present":true,"shell.activation_policy":"accessory","shell.hotkeys_skipped":true,"shell.hotkeys_registered":{"len":0},"window_verified":false,"route":null}' \
  opened \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"window_verified":true,"shell.window_kind":"popup","shell.background":"blurred","shell.route":"capture","shell.visible":true}' \
  'see:{"role":"Label","text":"Capture"}' \
  'absent:{"text":"Browse links"}' \
  a11y window_id screenshot:capture \
  'open_view:{"route":"browser"}' \
  'wait_state:{"window_verified":true,"shell.window_kind":"popup","shell.route":"browser"}' \
  'see:{"role":"Label","text":"Browse links"}' \
  screenshot:browser \
  'open_view:{"route":"capture"}' \
  opened
echo "exit $?"
```

Expected: fifteen JSON lines, each with `"ok": true`, then `exit 0`. Line 2 and the last
line show `"result": []` for `opened`. Line 3 (`trigger_hotkey`) already reports
`"route": "capture"`; line 4's state has `window_verified: true`, `window_kind: "popup"`,
`background: "blurred"`, `activation_policy: "accessory"` and `data_dir` equal to the
instance's own `data_dir` from `launch`. `window_id` is a positive number and each
`screenshot` line names a PNG under `$OUT`. Any `"ok": false` line ends the call with
exit 1 (the product did not do it); exit 2 is the harness (read stderr), exit 3 means
Screen Recording is missing for the screenshots.

Then record it (SKILL.md, Evidence) with `--feature shell`, every file in `$OUT` as an
artifact, and the `opened` sink plus the instance-owned `data_dir` as side effects.

## Proof

- Before any panel, `state` pins the defaults (`window_verified: false`, `route: null`)
  so the later `popup` / `blurred` / `window_verified: true` cannot be defaults: the
  panel was really created and read back from AppKit.
- After the hotkey path: `shell.window_kind == "popup"`, `shell.background ==
  "blurred"`, `shell.activation_policy == "accessory"` (no Dock icon), `shell.visible`,
  `shell.route == "capture"`, and the a11y tree shows a `Label` "Capture" and no
  "Browse links" node; the screenshot shows that panel.
- `open_view browser` switches the same panel's route (still verified, Label
  "Browse links"), and back.
- Side effects: the `opened` sink stays `[]` before and after (the shell launched
  nothing), and `state.data_dir` is the instance's own temp `data` dir, never
  `~/Library/Application Support`. With two instances, each reports its own `data_dir`
  and pid (doctor `endpoint` and `isolation`).

## Gotchas

- Phase 0 views are stubs: the panel shows only its route's title Label ("Capture",
  "Browse links"). Phase 1 replaces them; the shell checks above must keep holding.
- The panel hides when it loses key status, by design. If another app takes focus
  between steps (a notification, a concurrent GUI probe), `wait_state` on `visible`
  fails. Keep a whole proof inside one `call` (one desktop-lock hold) and re-run it
  once if the frontmost app changed; a second failure is a finding.
- `trigger_hotkey` with `UBILINK_SKIP_HOTKEYS=1` drives the hotkey handler, not the OS
  registration: `hotkeys_registered` is `[]` on purpose.
- `display_id` and the panel's position depend on the cursor's display; assert the
  route and flags, not where the panel landed.
- Screenshots need Screen Recording for the process running the harness; the a11y and
  state proofs do not. Without the grant, drop the `screenshot` steps and say so in
  `--observed`.
