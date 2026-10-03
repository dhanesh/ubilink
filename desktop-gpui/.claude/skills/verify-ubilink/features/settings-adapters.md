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
  'step:{"file":"{data_dir}/plugins-state.json","expect":{"disabled":{"contains":"arc"}}}' \
  'step:{"key_until":{"role":"Switch","text":"Arc","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"space"}' \
  'see:{"role":"Switch","text":"Arc","state":{"toggled":true}}' \
  'step:{"file":"{data_dir}/plugins-state.json","expect":{"disabled":{"not_contains":"arc"}}}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. `plugins-state.json` is
`ubilink-app`'s own format (`{"disabled": [lowercased app names]}`) in the instance's
own `{data_dir}`; it does not exist until the first toggle writes it. The full Phase
1 scenario (`probes/settings-adapters.json`) repeats this for all 8 non-built-in
adapters and restarts the instance twice (off survives, then on survives) — this
map's single round proves the write; see Gotchas for why restart stays out of this
skill's `call`.

## Proof

- **Switch reflects state.** Every adapter has a `Switch` whose accessible name is
  the app's own name; built-ins carry no switch to turn off.
- **Keyboard-operable.** Reached by `key_until` (no hard-coded focus sequence),
  toggled with Space.
- **Persisted.** Turning an adapter off adds its lowercased name to
  `plugins-state.json`'s `disabled` list in the instance's data dir; turning it back
  on removes it. That file is exactly what `ubilink-app`'s `list_plugins` reads on
  every launch, so writing it correctly is the persistence proof this skill can make
  without restarting the process (see Gotchas).

## Gotchas

- This skill's `call` refuses the `restart` step (SKILL.md's Drive section), so
  "survives a restart" is proven here by asserting the on-disk write directly rather
  than by actually stopping and relaunching the instance — stopping an instance
  removes its temp root entirely (SKILL.md's Cleanup section), so there is no way to
  restart the *same* process with the *same* data dir through this skill's public
  commands. `scripts/probe.py run settings-adapters` is the tool that drives the full
  restart-then-reread proof.
- `adapters.json` is cross-checked against `src-tauri/src/adapters.default.json` by
  `scripts/probe.py fixtures --check` (outside this skill); this map takes the
  fixture's adapter list as given rather than re-deriving it.
- Today's stub has no `Switch` nodes at all, so this call fails at the first `see`
  until Phase 1 lands.
