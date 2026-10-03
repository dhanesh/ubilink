# launch-at-login: A login-item switch backed by a fixture-fed seam

- id: launch-at-login
- proven: no
- anchors: src/views/settings.rs:SettingsView, src/login_item.rs:Status, src/login_item.rs:Report, src/login_item.rs:status, src/login_item.rs:register, src/login_item.rs:unregister, probes/launch-at-login.json, probes/fixtures/login-item-enabled.json, probes/fixtures/login-item-requires-approval.json, probes/fixtures/login-item-not-found.json

## What it is

Settings offers a "Launch at login" switch backed by `SMAppService.mainApp`, shown
through a status label that is exactly one of `Enabled`, `Requires approval`, `Not
registered` or `Not found`; the last two also show an explanation of what the status
means and what to do (spec U3). The shown status must survive a real restart. Phase
1 R13, proven through the fixture-fed seam `crate::login_item` (spec D6, ruling
R-o) rather than ever registering a real login item on this host.

## How to reach it

- **Tab to the switch, then Space** (the scenario's `key_until` step, then
  `key {"keys": "space"}`) to register or unregister.
- **Not driven here:** a real `SMAppService` round trip — that is an attended
  Phase 2 check (spec D6); the scenario loads a `login_item` fixture first so the
  seam, not `SMAppService`, answers every status/register/unregister.

## Drive it

The acceptance proof is the frozen scenario `probes/launch-at-login.json`, run
through the harness's `scenario` command. It drives all three statuses (enabled,
requires-approval, not-found), each round toggling on, restarting the instance in
place, confirming the status survived, then toggling off — not a single round.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name launch-at-login
echo "exit $?"
```

Expected: exit 0. `$OUT/launch-at-login/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows, for each of the three status rounds (`enabled`, `requires
approval`, `not found`), PASS lines for "...: shows ..." (the status label),
"...: one register call recorded" / "...: register call recorded", a `restart`
line, "...: still ... after the restart" / "...: same status and explanation after
the restart", and "...: unregister call recorded" / "...: no further seam call".

## Proof

- **Status text is exact.** Each round's "...: shows ..." step requires the label
  to read exactly one of `Enabled`, `Requires approval`, `Not registered`, `Not
  found` — never a paraphrase — and the switch is on only for
  `Enabled`/`Requires approval`.
- **Explanation shown verbatim.** For the `requires approval` and `not found`
  rounds, a second `Label` carries the fixture's `explanation` sentence exactly
  (U3).
- **Survives a real restart, every status.** Each round's `restart` step
  relaunches the same binary with the same data dir (the `login_item` fixture copy
  kept); the following "...: still ... after the restart" step reopens Settings
  and requires the same status (and explanation) to still show — proving the
  status is read fresh from the replayed fixture copy, not cached in memory across
  the restart, for all three statuses, not just one.
- **The seam, not `SMAppService`, answers.** Every register/unregister appends its
  name to the fixture copy's `calls` list (a `file` step reads that back) — proof
  the view asked `crate::login_item`, not the real OS service, for this instance.
- **No side effect outside the sandbox.** Because the seam intercepts every call,
  no real login item is ever registered on the host, whatever this proof does.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.

Record it (SKILL.md, Evidence) — the scenario's three restarts are captured inside
`$OUT/launch-at-login/` as per-step artifacts, so no separate restart record is
needed:

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/launch-at-login"
RESULT=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print("pass" if r["exit"]==0 else "fail")' "$R/result.json")
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature launch-at-login \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario launch-at-login (desktop-gpui/probes/launch-at-login.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "the seam's status and calls survive a real restart for all three statuses; opened sink stayed []; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `scenario`'s `restart` steps are probe.py's own, run inside the scenario — not
  the harness's separate `restart` command (SKILL.md's "Proving persistence"
  section is for hand-written `call` sequences only); a verifier never issues
  `restart` by hand for this feature.
- `Some(..)` from the seam happens only in an automation build with a `login_item`
  fixture loaded; a release build always gets `None` and must call `SMAppService`
  itself — this proof says nothing about that path (Phase 2, spec D6).
- The view must re-read status every time Settings is shown, not cache it across
  shows — the scenario's close/reopen between rounds is how it catches a cached
  read.
- Today's stub has no "Launch at login" switch or status label at all, so
  `scenario --name launch-at-login` exits 1 at the first `see` until Phase 1 lands.
