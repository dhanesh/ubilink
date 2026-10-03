# shell: Menu-bar app and its pop-up panel

- id: shell
- proven: a386051b2b066316b46417aee4038bef0c2afc10
- anchors: src/shell/mod.rs:handle_event, src/shell/panel.rs:PanelView, src/shell/state.rs:ShellState, src/shell/tray.rs:command_for, src/shell/hotkeys.rs:bindings, src/automation/mod.rs:state_json, src/paths.rs, probes/shell.json

## What it is

ubilink lives in the menu bar, not the Dock. Pressing a global hotkey (or choosing an
item in the tray menu) pops up one translucent, blurred panel near the cursor, showing
the view that hotkey belongs to: Capture, Browse links or Settings. Switching views
reuses the same panel; nothing else opens or launches. Every view built on top of the
shell (capture, browser, settings, onboarding) depends on this working first.

## How to reach it

- **Global hotkeys** (`src/shell/hotkeys.rs`, `bindings`): Cmd+Shift+K opens capture,
  Cmd+Shift+L opens the link browser, Cmd+Shift+, opens settings. Automation instances
  skip registering them (`UBILINK_SKIP_HOTKEYS=1`), so the verifier drives the same code
  path with `trigger_hotkey {"action": "capture" | "browser" | "settings"}`. That posts
  the event into `shell::handle_event` exactly where a real Carbon hotkey event enters.
  The proof drives all three.
- **Tray menu items** `capture`, `browse`, `settings` (`src/shell/tray.rs`,
  `command_for`) map to `Command::Open(route)`. That is the same command the `open_view`
  verb executes, and the proof drives it.
- **Not driven here:**
  - the tray's `quit` item: there is no automation verb for it, and Cleanup's stop
    covers ending the process
  - clicking the tray icon itself
  - the three real OS hotkey registrations, which are a maintainer's attended check
    (probes/shell.json notes), because an automation instance must never grab the
    user's keys

## Drive it

On a launched instance, after a passing doctor (SKILL.md, Launch and Doctor), in `$WT`
with the session variables set. The whole proof, side effects included, is one `call`:

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/shell"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  pasteboard:set_nonce \
  'wait_state:{"shell.tray_present":true,"shell.activation_policy":"accessory","shell.hotkeys_skipped":true,"shell.hotkeys_registered":{"len":0},"shell.visible":false}' \
  opened \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"window_verified":true,"shell.window_kind":"popup","shell.background":"blurred","shell.route":"capture","shell.visible":true}' \
  'see:{"role":"Label","text":"Capture"}' \
  'absent:{"text":"Browse links"}' \
  'step:{"winframe":"any"}' \
  a11y screenshot:capture \
  'step:{"call":"resign_key","expect":{"route":"capture","closed":true,"state.shell.visible":false,"state.route":null}}' \
  'step:{"winframe":"gone"}' \
  'trigger_hotkey:{"action":"browser"}' \
  'wait_state:{"window_verified":true,"shell.window_kind":"popup","shell.background":"blurred","shell.route":"browser","shell.visible":true}' \
  'see:{"role":"Label","text":"Browse links"}' \
  'step:{"winframe":"any"}' \
  screenshot:browser \
  'step:{"activate_other":true}' \
  'wait_state:{"shell.visible":false,"route":null}' \
  'step:{"winframe":"gone"}' \
  'trigger_hotkey:{"action":"settings"}' \
  'wait_state:{"window_verified":true,"shell.window_kind":"popup","shell.route":"settings","shell.visible":true}' \
  'step:{"winframe":"any"}' \
  screenshot:settings \
  'step:{"call":"resign_key","expect":{"route":"settings","closed":false,"state.shell.visible":true,"state.route":"settings"}}' \
  'step:{"activate_other":true,"settle_ms":800}' \
  'step:{"call":"state","expect":{"shell.visible":true,"route":"settings"}}' \
  'step:{"winframe":"any"}' \
  'open_view:{"route":"browser"}' \
  'wait_state:{"window_verified":true,"shell.route":"browser","shell.visible":true}' \
  'see:{"role":"Label","text":"Browse links"}' \
  opened \
  'pasteboard:{"changed":false}'
echo "exit $?"
```

Expected output: 34 JSON lines, each with `"ok": true`, then `exit 0`. The first line is
`reset_panel`; the steps follow, numbered 2 to 34. The two `opened` lines show
`"result": []`. Each `screenshot` line names a PNG under `$OUT`, and the last line
reports the pasteboard's change count unchanged.

- Any `"ok": false` line ends the call with exit 1: the product did not do it.
- A line with `"outcome": "focus-stolen-retry"` means another app took focus. The
  proof re-runs once with `"attempt": 2`, and a second theft exits 3 (ENV-BLOCKED,
  not a product failure).
- Exit 2 is the harness: read stderr.
- Exit 3 can also mean Screen Recording is missing for the screenshots.

Then record it (SKILL.md, Evidence) with `--worktree "$WT" --feature shell`, passing
every file in `$OUT` as an artifact. The side effects to name are the `opened` sink, the
unchanged pasteboard and the instance-owned `data_dir`.

## Proof

- **The flags are not defaults.** Before any panel, `state` shows tray present,
  accessory policy, hotkeys skipped, none registered, and not visible. The proof starts
  from a closed panel (`reset_panel`). So the later `popup`, `blurred` and
  `window_verified: true` are read back from a real AppKit panel; they are not defaults.
- **Capture (hotkey path).** The panel is a verified blurred popup on route `capture`,
  on screen (Quartz `winframe any`). The a11y tree has a `Label` "Capture" and no
  "Browse links". `resign_key` (the click-away observer's code) closes it: `closed:
  true`, not visible, and its window left Quartz's on-screen list (`winframe gone`).
- **Browser (hotkey path).** The panel is a verified blurred popup on route `browser`,
  with Label "Browse links". A real loss of key status hides it, with its window gone.
  That loss comes from probe.py's activator app becoming active.
- **Settings (hotkey path).** The panel is a verified popup on route `settings`. Neither
  `resign_key` (`closed: false`) nor a real deactivation hides it: it is still visible
  and still on screen.
- **Tray Open path.** `open_view browser` shows the browser route in the same verified
  panel.
- **Side effects.** The `opened` sink stays `[]`, so the shell launched nothing. The
  pasteboard change count is unchanged, so the shell copied nothing. `state.data_dir` is
  the instance's own temp `data` dir. With two instances, each reports its own
  `data_dir` and pid (doctor's `endpoint` and `isolation` checks).

## Gotchas

- Phase 0 views are stubs: the panel shows only its route's title Label ("Capture",
  "Browse links"). Phase 1 replaces them; the shell checks above must keep holding.
- The capture and browser panels hide when they lose key status, by design. The
  harness re-runs the whole proof once from a closed panel if another app took focus
  during a visibility step, and exits 3 (ENV-BLOCKED) if focus is taken again. A
  failure with no focus theft is exit 1, a product failure.
- On this Phase 0 build, `a11y` on the settings panel answers "no accessibility frame
  ... active=false": the panel stays open without becoming the active window. That is
  why the settings part of the proof uses `state`, Quartz `winframe` and a screenshot,
  not `see`. Phase 1's settings view (U4) must make it readable.
- `trigger_hotkey` with `UBILINK_SKIP_HOTKEYS=1` drives the hotkey handler, not the OS
  registration: `hotkeys_registered` is `[]` on purpose.
- `display_id` and the panel's position depend on the cursor's display; assert the
  route and flags, not where the panel landed.
- Screenshots need Screen Recording for the process running the harness; the a11y and
  state proofs do not. Without the grant, drop the `screenshot` steps and say so in
  `--observed`.
