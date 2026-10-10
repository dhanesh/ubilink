# copy-link: Copying the captured link takes at most two actions

- id: copy-link
- proven: no
- anchors: probes/copy-link.json, probes/copy-dismiss.json, probes/fixtures/capture-ok.json
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
SCENARIOS="copy-link copy-dismiss"
for name in $SCENARIOS; do
  python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name "$name"
  echo "$name exit $?"
done
```

Expected: both exit 0. `$OUT/copy-link/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows a PASS line for each of "Enter: pbpaste prints the fixture
URI, change count higher" and "Cmd-C: pbpaste prints the fixture URI, change count
higher", with a "fresh instance for the Cmd-C round" `restart` line between them.

`$OUT/copy-dismiss/result.json`'s `verdict` is `"PASS"`; `transcript.txt` shows PASS
for "Enter: still visible on capture ~1 s after the copy", "Enter: the panel hides by
itself within ~3 s of the key", "Enter: window left the screen" and the same three
"Cmd-C: ..." lines.

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
- **Self-dismiss (Phase 2 parity, `copy-dismiss`).** After each copy key the Copied
  Label is still shown and the panel still visible about 1 s later, then within about
  3 s of the key the panel hides by itself: `shell.visible` false and route null,
  `window_id` refuses, and `winframe: gone` sees the window leave Quartz's on-screen
  list. The pasteboard still holds the URI afterwards.

Record it (SKILL.md, Evidence). Each feature has one evidence record per head, so
both scenarios go into **one** `verify_evidence.py record` call, each scenario's
artifacts renamed to a unique prefix first, and `--result` is `fail` if **either**
scenario's exit is not 0 (browser-navigation.md explains why).

```sh
python3 "$H" doctor --instance "$INSTANCE"
mkdir -p "$WT/.verify-run/$INSTANCE"
REC="$(mktemp -d "$WT/.verify-run/$INSTANCE/record-copy-link.XXXXXX")"
RESULT=pass
for name in $SCENARIOS; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature copy-link \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario(s) $SCENARIOS (desktop-gpui/probes/<name>.json) on instance $INSTANCE" \
  --observed "$(for name in $SCENARIOS; do printf '%s: %s; ' "$name" "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/$name/result.json")"; done)" \
  --side-effect "pasteboard carries the fixture's exact URI after each round of both scenarios; opened sink stayed []; see each scenario's renamed result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
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
- `copy-dismiss` is a Phase 2 parity scenario: on the Phase 1 head (5017f5b) it exits
  1 at "Enter: the panel hides by itself within ~3 s of the key" (the panel stays
  open after a copy) until Phase 2 lands. Its timing is from the key press and
  approximate; an implementation should hold Copied about 1.2-1.5 s.
