# copy-link: Copying the captured link takes at most two actions

- id: copy-link
- proven: no
- anchors: src/views/capture.rs:CaptureView, probes/copy-link.json, probes/fixtures/capture-ok.json
- constraints: linking-manifesto-app:U2

## What it is

Once a resource is captured, pressing Enter or Cmd-C puts its URI on the macOS
pasteboard and shows a "Copied" confirmation, kept on screen at least a second even
if the panel then dismisses itself. Counting from the hotkey, that is exactly two
user actions (the hotkey, then one key) — Phase 1 R2 / spec U2's ceiling.

## How to reach it

- **Enter** and **Cmd-C** (`key {"keys": "enter"}`, `key {"keys": "cmd-c"}`) while
  the capture view shows a captured resource. Both must work independently; the
  Phase 1 scenario drives Enter first, then a fresh instance for Cmd-C so neither
  round depends on the other leaving the panel open.
- **Not driven here:** any mouse action — copying is keyboard-only by design.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/copy-link"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'load_fixture:{"kind":"capture","path":"{fixtures}/capture-ok.json"}' \
  pasteboard:set_nonce \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture"}' \
  'see:{"role":"Label","text":"Example Domain"}' \
  'absent:{"text":"Copied"}' \
  'key:{"keys":"enter"}' \
  'see:{"role":"Label","text":"Copied"}' \
  'pasteboard:{"changed":true}' \
  opened
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0; the final `opened` line reports
`"result": []`. Repeat the same sequence on a second instance, substituting
`'key:{"keys":"cmd-c"}'` for the Enter step, to cover the Cmd-C path independently
(a fresh instance per the Phase 1 scenario's own restart-between-rounds design,
`probes/copy-link.json`).

## Proof

- **Two actions.** Between `trigger_hotkey` and the key step, no other action is
  sent — only observations (`see`, `pasteboard`) — so the recorded call itself is
  the "at most two actions" proof.
- **Pasteboard.** `pasteboard:{"changed":true}` requires the change count to have
  risen since the `set_nonce` baseline; record also captures the exact URI text with
  `pbpaste` outside the call (Evidence section) since the pasteboard step here only
  asserts the change, not the content — the Phase 1 scenario
  (`probes/copy-link.json`) additionally reads back the fixture's exact URI with
  `pasteboard: read, expect: ...`.
- **Confirmation.** An a11y `Label` "Copied" appears after the key and must stay
  visible at least a second if the view then dismisses the panel (README's Phase 1
  view-state contract) — this call's `see` step only has to catch it once, since the
  one-second hold is a timing guarantee the harness does not independently clock.
- **Side effect.** The `opened` sink stays `[]`: copying never launches anything.

## Gotchas

- `pasteboard:set_nonce` / `pasteboard:{"changed":true}` only track the pasteboard's
  **change count**, not its text; when the exact URI matters for the record, follow
  with `pasteboard:{"expect": "..."}` (a literal or `{nonce}`) the way
  `probes/copy-link.json` reads the fixture's URI back with `pbpaste`.
- This map deliberately asserts the fixture's `name` field ("Example Domain"), not
  its URI, in the command above, so the literal fixture domain is never baked into
  this file.
- Today's stub never reaches a `captured` phase (phase stays `"stub"`), so this call
  fails at the first `see` until Phase 1 lands.
