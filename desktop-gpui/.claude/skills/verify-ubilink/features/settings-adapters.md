# settings-adapters: Every adapter toggles from the keyboard and persists

- id: settings-adapters
- proven: no
- anchors: src/views/settings.rs:SettingsView, probes/settings-adapters.json, probes/fixtures/adapters.json
- constraints: linking-manifesto-app:T5

## What it is

Settings lists every resource adapter (Arc, Bear, Brave Browser, ...) as a switch
whose accessible name is the app's name exactly. Toggling one off with the keyboard
writes `plugins-state.json` in the instance's data dir; toggling it back on removes
it from that file. Built-in adapters (Finder, Safari) have no switch to turn off.
Phase 1 R10; spec T5 ("resource-addressing behind a platform/app adapter interface" —
each adapter carries its own URI scheme, `probes/fixtures/adapters.json`'s `scheme`
field).

## How to reach it

- **The tray's Open on `settings`** (`open_view {"route": "settings"}`).
- **Tab to a switch, then Space** (`step:{"key_until": {...}, "key": "tab"}`, then
  `key {"keys": "space"}`) to toggle it.
- **Not driven here:** Export/Import/Reset (`graph-data.md`), the launch-at-login
  switch (`launch-at-login.md`) — same view, different controls.

## Drive it

Three commands: a `call` that toggles off, a real `restart` (SKILL.md's "Proving
persistence" section), then a `call` that confirms the switch is still off and
toggles it back on, then a second `restart` and a third `call` confirming it is on
again.

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/settings-adapters"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'open_view:{"route":"settings"}' \
  'wait_state:{"shell.visible":true,"route":"settings"}' \
  'see:{"role":"Switch","text":"Arc","state":{"toggled":true}}' \
  'step:{"file":"{data_dir}/plugins-state.json","exists":false}' \
  'step:{"key_until":{"role":"Switch","text":"Arc","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"space"}' \
  'see:{"role":"Switch","text":"Arc","state":{"toggled":false}}' \
  'step:{"file":"{data_dir}/plugins-state.json","expect":{"disabled":{"contains":"arc"}}}'
echo "off exit $?"

python3 "$H" restart --instance "$INSTANCE" --verifier "$VERIFIER"
echo "restart exit $?"

python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'open_view:{"route":"settings"}' \
  'wait_state:{"shell.visible":true,"route":"settings"}' \
  'see:{"role":"Switch","text":"Arc","state":{"toggled":false}}' \
  'step:{"key_until":{"role":"Switch","text":"Arc","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"space"}' \
  'see:{"role":"Switch","text":"Arc","state":{"toggled":true}}' \
  'step:{"file":"{data_dir}/plugins-state.json","expect":{"disabled":{"not_contains":"arc"}}}'
echo "on exit $?"

python3 "$H" restart --instance "$INSTANCE" --verifier "$VERIFIER"
echo "restart exit $?"

python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'open_view:{"route":"settings"}' \
  'see:{"role":"Switch","text":"Arc","state":{"toggled":true}}'
echo "survives-on exit $?"
```

Expected: every `call` prints `"ok": true` lines and exits 0; each `restart` prints
`{"RESTARTED": {..., "data_dir_kept": true, ...}}` and exits 0 (it keeps the temp
root, so `plugins-state.json` and the fixture copy survive; it replays no fixture
here since none was loaded). `plugins-state.json` is `ubilink-app`'s own format
(`{"disabled": [lowercased app names]}`) in the instance's `{data_dir}`; it does not
exist until the first toggle writes it. Run the doctor again after each `restart`
(SKILL.md: `process` must now match the new pid) before recording. The full Phase 1
scenario (`probes/settings-adapters.json`) repeats this shape for all 8 non-built-in
adapters.

## Proof

- **Switch reflects state.** Every adapter has a `Switch` whose accessible name is
  the app's own name; built-ins carry no switch to turn off.
- **Keyboard-operable.** Reached by `key_until` (no hard-coded focus sequence),
  toggled with Space.
- **Survives a real restart, off then on.** After the off-toggle and a real
  `restart` (same binary, same data dir, fixtures replayed), the switch still reads
  off and `plugins-state.json` still lists the adapter as disabled; after toggling
  it on and a second `restart`, the switch reads on and the adapter is gone from
  `disabled`. This is the app's own startup read of `plugins-state.json`, not an
  assertion about the file alone.

## Gotchas

- `restart` is a harness command between two `call`s (SKILL.md's "Proving
  persistence" section), not a step inside `call` — `call` itself still refuses a
  `restart` step. It quits the app by its recorded pid, keeps the temp root (data
  dir, fixture copy, log), relaunches the same binary and replays every fixture
  loaded so far; it refuses (exit 2) if the binary changed since launch or the root
  is gone.
- `adapters.json` is cross-checked against `src-tauri/src/adapters.default.json` by
  `scripts/probe.py fixtures --check` (outside this skill); this map takes the
  fixture's adapter list as given rather than re-deriving it.
- Today's stub has no `Switch` nodes at all, so this call fails at the first `see`
  until Phase 1 lands.
