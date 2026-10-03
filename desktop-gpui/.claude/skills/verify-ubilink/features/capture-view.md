# capture-view: The capture view shows what you just copied, render-first

- id: capture-view
- proven: no
- anchors: src/views/capture.rs:CaptureView, src/views/capture.rs:State, src/views/mod.rs:ViewState, src/fixtures.rs:Capture, probes/capture-view.json, probes/fixtures/capture-delayed.json, probes/fixtures/capture-failed.json
- constraints: linking-manifesto-app:B1, linking-manifesto-app:U3

## What it is

Pressing the capture hotkey shows the panel immediately in a "capturing" state —
before the core has even answered — then fills in with the frontmost resource's name
and URI once the capture resolves. When the capture fails, the view shows a sentence
that names what failed and what to do about it, never a generic error. This is Phase
1 R1: render-first so U1's latency budget survives even while a live capture is
pending (spec TN1).

## How to reach it

- **The capture hotkey** (`trigger_hotkey {"action": "capture"}`), the same entry
  point `shell.md` already proves for the Phase 0 stub. Phase 1 replaces the stub's
  body in `src/views/capture.rs`; the hotkey path itself is unchanged.
- **Not driven here:** a real `osascript` capture of the frontmost app. Automation
  builds hand the view a `load_fixture {"kind": "capture", ...}` result instead (a
  production boundary the fixture stands in for — probes/fixtures/README.md).

## Drive it

On a launched instance, after a passing doctor, in `$WT` with the session variables
set (SKILL.md). Two calls: one for the delayed-success path (the render-first and
filled-in proofs), one for the failure path.

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/capture-view"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'load_fixture:{"kind":"capture","path":"{fixtures}/capture-delayed.json"}' \
  'wait_state:{"route":null}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture","view.phase":"capturing"}' \
  'see:{"role":"Label","text":"Capturing"}' \
  'see:{"role":"Label","text":"Delayed Capture Target"}' \
  'wait_state:{"view.phase":"captured"}' \
  'absent:{"text":"Capturing"}' \
  'load_fixture:{"kind":"capture","path":"{fixtures}/capture-failed.json"}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":false,"route":null}' \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"shell.visible":true,"route":"capture"}' \
  'see:{"role":"Label","text":"Safari returned no active resource — open something there and try again.","exact":true}' \
  'absent:{"text":"Delayed Capture Target"}' \
  'wait_state:{"view.phase":"failed"}' \
  opened
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0. The `Capturing` label appears
within the harness's default step timeout while the 1-second-delayed fixture is still
pending (`probes/fixtures/capture-delayed.json`'s `delay_ms`); the `captured` phase
and the name label appear once it resolves (the Phase 1 scenario,
`probes/capture-view.json`, additionally asserts the fixture's URI label the same
way — omitted from this literal command only to avoid baking a fixture URL into this
map). The error path shows the failed fixture's exact sentence
(`probes/fixtures/capture-failed.json`'s `error`), and the final `opened` line
reports `"result": []`.

## Proof

- **Render-first.** `view.phase` is `"capturing"` in the same reply that opens the
  panel (before the fixture's delay elapses), and an a11y `Label` "Capturing" is on
  screen while the capture is still pending — the only way to observe this is to read
  both before the delay runs out (TN1).
- **Captured.** Once resolved, `view.phase` is `"captured"`, the "Capturing" label is
  gone, and the a11y tree holds `Label`s for the fixture's name and URI (the capture
  truncates long URIs with an ellipsis; the harness's text matcher still matches the
  fixture's text against a truncated label — probes/README.md's text-matching rule).
- **Failure (U3).** On a failing fixture, an a11y `Label` states the fixture's
  sentence verbatim — what failed and what to do — with no stale capture data left
  showing, and `view.phase` is `"failed"`.
- **Side effect.** The `opened` sink stays `[]`: nothing is launched by a capture,
  success or failure.

## Gotchas

- The "Capturing" label must be read before the fixture's delay elapses, so this
  call's step timeout for that `see` must stay under the fixture's `delay_ms` (the
  Phase 1 scenario uses an 800 ms window inside the fixture's 1000 ms delay,
  probes/capture-view.json) — too generous a timeout can pass even if the view skips
  straight to `captured` and never renders the pending state at all.
- A narrow panel truncates long URIs with an ellipsis; assert on enough of the text
  for probes/README.md's truncation rule to still match (at least 12 characters, or
  half the string), not the bare domain.
- This feature is Phase 1 work: today's stub always reports `phase: "stub"` and never
  shows a Capturing/captured/failed sequence, so this call fails at the first
  `wait_state` until Phase 1 lands. `proven: no` stays until then.
