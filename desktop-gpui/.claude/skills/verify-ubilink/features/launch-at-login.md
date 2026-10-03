# launch-at-login: A login-item switch backed by a fixture-fed seam

- id: launch-at-login
- proven: no
- anchors: src/views/settings.rs:SettingsView, src/login_item.rs:Status, src/login_item.rs:Report, src/login_item.rs:status, src/login_item.rs:register, src/login_item.rs:unregister, probes/launch-at-login.json, probes/fixtures/login-item-enabled.json, probes/fixtures/login-item-requires-approval.json, probes/fixtures/login-item-not-found.json

## What it is

Settings offers a "Launch at login" switch backed by `SMAppService.mainApp`, shown
through a status label that is exactly one of `Enabled`, `Requires approval`, `Not
registered` or `Not found`; the last two also show an explanation of what the status
means and what to do (spec U3). Phase 1 R13, proven through the fixture-fed seam
`crate::login_item` (spec D6, ruling R-o) rather than ever registering a real login
item on this host.

## How to reach it

- **Tab to the switch, then Space** (`step:{"key_until": {...}}`, then
  `key {"keys": "space"}`) to register or unregister.
- **Not driven here:** a real `SMAppService` round trip — that is an attended
  Phase 2 check (spec D6); every call here loads a `login_item` fixture first so the
  seam, not `SMAppService`, answers every status/register/unregister.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/launch-at-login"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'load_fixture:{"kind":"login_item","path":"{fixtures}/login-item-enabled.json"}' \
  'open_view:{"route":"settings"}' \
  'wait_state:{"shell.visible":true,"route":"settings"}' \
  'see:{"role":"Switch","text":"Launch at login","state":{"toggled":false}}' \
  'see:{"role":"Label","text":"Not registered"}' \
  'step:{"file":"{fixtures}/login-item-enabled.json","expect":{"calls":{"exists":false}}}' \
  'step:{"key_until":{"role":"Switch","text":"Launch at login","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"space"}' \
  'see:{"role":"Label","text":"Enabled","exact":true}' \
  'step:{"file":"{fixtures}/login-item-enabled.json","expect":{"calls":{"contains":"register"}}}'
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. The two `requires_approval` and
`not_found` rounds (not reproduced here) follow the same shape against
`login-item-requires-approval.json` and `login-item-not-found.json`, each asserting
the label's exact status text plus the fixture's `explanation` verbatim
(`probes/launch-at-login.json`); the switch stays off for `not_found` even after
toggling on, since that status is not a registered one.

## Proof

- **Status text is exact.** The label reads one of `Enabled`, `Requires approval`,
  `Not registered`, `Not found` — never a paraphrase — and the switch is on only for
  `Enabled`/`Requires approval`.
- **Explanation shown verbatim.** For `requires_approval` and `not_found`, a second
  `Label` carries the fixture's `explanation` sentence exactly (U3).
- **The seam, not `SMAppService`, answers.** Every register/unregister appends its
  name to the fixture copy's `calls` list (`step:{"file": ...}` reads that back) —
  proof the view asked `crate::login_item`, not the real OS service, for this
  instance.
- **No side effect outside the sandbox.** Because the seam intercepts every call,
  no real login item is ever registered on the host, whatever this proof does.

## Gotchas

- This skill's `call` refuses the `restart` step, so "the status survives a
  restart" — which the Phase 1 scenario proves by actually restarting the instance
  and reopening Settings — is out of reach for a single `call` here; this map proves
  the seam wrote the right status and call to the fixture copy, which is exactly
  what a restart reads back. `scripts/probe.py run launch-at-login` drives the
  restart itself.
- `Some(..)` from the seam happens only in an automation build with a `login_item`
  fixture loaded; a release build always gets `None` and must call `SMAppService`
  itself — this proof says nothing about that path (Phase 2, spec D6).
- The view must re-read status every time Settings is shown, not cache it across
  shows — a close/reopen between fixture swaps (not in this short command) is how
  the Phase 1 scenario catches a cached read.
- Today's stub has no "Launch at login" switch or status label at all, so this call
  fails at the first `see` until Phase 1 lands.
