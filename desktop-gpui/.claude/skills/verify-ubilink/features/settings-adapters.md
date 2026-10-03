# settings-adapters: Every adapter toggles from the keyboard and persists

- id: settings-adapters
- proven: no
- anchors: src/views/settings.rs:SettingsView, probes/settings-adapters.json, probes/fixtures/adapters.json

## What it is

Settings lists every resource adapter (Arc, Bear, Brave Browser, ...) as a switch
whose accessible name is the app's name exactly. Toggling one off with the keyboard
writes `plugins-state.json` in the instance's data dir; toggling it back on removes
it from that file. Built-in adapters (Finder, Safari) have no switch to turn off.
Each must survive a real restart, off then on. Phase 1 R10.

## How to reach it

- **The tray's Open on `settings`** (`open_view {"route": "settings"}`).
- **Tab to a switch, then Space** (the scenario's `key_until` step, then
  `key {"keys": "space"}`) to toggle it.
- **Not driven here:** Export/Import/Reset (`graph-data.md`), the launch-at-login
  switch (`launch-at-login.md`) — same view, different controls.

## Drive it

The acceptance proof is the frozen scenario `probes/settings-adapters.json`, run
through the harness's `scenario` command. It toggles all 8 non-built-in adapters
off, restarts the instance in place, confirms each is still off, toggles all 8 back
on, restarts again, and confirms each is still on — every adapter, not a sample.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name settings-adapters
echo "exit $?"
```

Expected: exit 0. `$OUT/settings-adapters/result.json`'s `verdict` is `"PASS"` and
`counts.passed` covers all 8 adapters' off round, the restart, the off-survives
check, the on round, the second restart, and the on-survives check (one PASS triplet
per adapter per direction in `transcript.txt`, e.g. "off 1/8: Arc switch off", "off
1/8: written to plugins-state.json", through "on 8/8: removed from
plugins-state.json", plus "every adapter still off; built-ins on" and "every
adapter still on" after each restart).

## Proof

- **Switch reflects state.** Every adapter has a `Switch` whose accessible name is
  the app's own name; built-ins carry no switch to turn off.
- **Keyboard-operable.** Each adapter is reached by `key_until` (no hard-coded focus
  sequence) and toggled with Space.
- **Survives a real restart, off then on, for every adapter.** After all 8 are
  toggled off, the scenario's `restart` step relaunches the same binary with the
  same data dir; "every adapter still off; built-ins on" re-opens Settings and
  re-reads every switch. The same holds in reverse after toggling back on and a
  second restart. This is the app's own startup read of `plugins-state.json`
  (`ubilink-app`'s `{"disabled": [lowercased app names]}` format), not an assertion
  about the file alone.

Record it (SKILL.md, Evidence) — the scenario's two restarts are captured inside
`$OUT/settings-adapters/` as per-step artifacts, so no separate restart record is
needed:

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/settings-adapters"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature settings-adapters \
  --verifier "$VERIFIER" --result pass \
  --action "scenario settings-adapters (desktop-gpui/probes/settings-adapters.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "plugins-state.json's disabled list matches every toggle, before and after two real restarts; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `scenario`'s `restart` steps are probe.py's own, run inside the scenario — not
  the harness's separate `restart` command (SKILL.md's "Proving persistence"
  section is for hand-written `call` sequences only); a verifier never issues
  `restart` by hand for this feature.
- `adapters.json` is cross-checked against `src-tauri/src/adapters.default.json` by
  `scripts/probe.py fixtures --check` (outside this skill); this scenario takes the
  fixture's adapter list as given rather than re-deriving it.
- Today's stub has no `Switch` nodes at all, so `scenario --name settings-adapters`
  exits 1 at the first `see` until Phase 1 lands.
