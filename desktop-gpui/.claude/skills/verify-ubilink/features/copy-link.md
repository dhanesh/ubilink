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
  frozen scenario drives Enter first, then restarts the instance for the Cmd-C round
  so neither depends on the other leaving the panel open.
- **Not driven here:** any mouse action — copying is keyboard-only by design.

## Drive it

The acceptance proof is the frozen scenario `probes/copy-link.json`, run with the
harness's `scenario` command (SKILL.md's Drive section). It drives both rounds (Enter,
then Cmd-C after an in-scenario `restart`) and checks the pasteboard's actual content,
not just that it changed.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name copy-link
echo "exit $?"
```

Expected: exit 0. `$OUT/copy-link/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows a PASS line for each of "Enter: pbpaste prints the fixture
URI, change count higher" and "Cmd-C: pbpaste prints the fixture URI, change count
higher", with a "fresh instance for the Cmd-C round" `restart` line between them.

## Proof

- **Two actions, each round.** Between `trigger_hotkey` and the key step in each
  round, the scenario only observes (`see`, `pasteboard`) — no other input — so the
  scenario's own shape is the "at most two actions" proof.
- **Pasteboard holds the exact URI.** "Enter/Cmd-C: pbpaste prints the fixture URI,
  change count higher" is a `pasteboard: read` step whose `expect` is the fixture's
  literal URI (not merely `changed: true`) and whose `changed: true` requires the
  change count to have risen since that round's `set_nonce` — proving the content,
  not only that something happened.
- **Confirmation.** "Enter/Cmd-C: a11y Copied confirmation" requires a `Label`
  "Copied" after the key, in both rounds.
- **Independence.** The Cmd-C round runs after a real `restart` (fresh process,
  `capture-ok.json` replayed), so it cannot be passing only because the Enter round
  left the panel in a convenient state.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/copy-link"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature copy-link \
  --verifier "$VERIFIER" --result pass \
  --action "scenario copy-link (desktop-gpui/probes/copy-link.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "pasteboard carries the fixture's exact URI after each round; opened sink stayed []; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `scenario` replaces the instance's process and temp root with a fresh one before
  it runs, and the scenario's own `restart` step (run by probe.py inside
  `scenario`, not a separate harness command) relaunches it again mid-run — do not
  assume this instance's process is the one `launch` started by the time the
  scenario finishes.
- `pasteboard: set_nonce` saves your real pasteboard text and restores it when the
  scenario's run ends, not after each round — do not read the pasteboard between
  the two rounds expecting your own content back.
- This feature is Phase 1 work: today's stub never reaches a `captured` phase
  (phase stays `"stub"`), so `scenario --name copy-link` exits 1 at its first `see`
  until Phase 1 lands.
