# capture-view: The capture view shows what you just copied, render-first

- id: capture-view
- proven: no
- anchors: src/views/capture.rs:CaptureView, src/views/capture.rs:State, src/views/mod.rs:ViewState, src/fixtures.rs, probes/capture-view.json, probes/capture-permission-route.json, probes/fixtures/capture-delayed.json, probes/fixtures/capture-failed.json
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

The acceptance proof is the frozen scenario `probes/capture-view.json`, run through
the harness's `scenario` command (SKILL.md's Drive section), not a hand-written
`call`. It checks every obligation R1 states — the render-first phase, the filled-in
name and URI, the error sentence, the `opened` sink — not a sample of them.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
SCENARIOS="capture-view capture-permission-route"
for name in $SCENARIOS; do
  python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name "$name"
  echo "$name exit $?"
done
```

Expected: both exit 0. `$OUT/capture-view/result.json`'s `verdict` is `"PASS"` and
`counts.failed` is 0; `transcript.txt` shows one PASS line per step, including "a11y
Capturing node while the capture is pending", "captured: name and URI in the a11y
tree", "a11y error node states the failure and the next step (verbatim fixture
text)" and "nothing launched". The per-step JSON files under `$OUT/capture-view/`
hold each step's request/response, so a verifier can read the exact `view.phase`
value and a11y dump at each point without re-driving it.

`$OUT/capture-permission-route/result.json`'s `verdict` is `"PASS"`; `transcript.txt`
shows PASS for "automation missing: the failed capture routes to onboarding",
"automation missing: same window id", "automation granted: still on capture, phase
failed" and "automation granted: no onboarding steps".

## Proof

- **Render-first.** The scenario's "hotkey opens the view already in capturing"
  step requires `view.phase: "capturing"` in the same reply that opens the panel
  (before `capture-delayed.json`'s 1-second delay elapses), and the following
  "a11y Capturing node while the capture is pending" step (`timeout_ms: 800`, inside
  that 1-second window) requires an a11y `Label` "Capturing" to already be on
  screen — the pairing is what proves the pending state is actually rendered, not
  skipped.
- **Captured.** "captured: name and URI in the a11y tree" requires `Label`s for
  both the fixture's name ("Delayed Capture Target") and its URI; "capturing
  indicator gone once captured" requires "Capturing" to be absent; the `state` call
  right after requires `view.phase: "captured"`.
- **Failure (U3).** After `load_fixture capture-failed.json` and a fresh
  `trigger_hotkey`, "a11y error node states the failure and the next step (verbatim
  fixture text)" requires the a11y `Label`'s text to equal
  `capture-failed.json`'s `error` sentence exactly (`exact: true`); "no stale
  capture shown with the error" requires the prior fixture's name to be absent;
  `view.phase` reads `"failed"`.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.
- **Permission route (Phase 2 parity, `capture-permission-route`).** A failed capture
  whose permission report (the `permission` fixture, the one onboarding reads) says
  Automation is not granted switches the same window (saved `window_id`, unchanged
  process) to route `onboarding` with its steps listed and nothing opened; after a
  `restart` with `permissions-all-granted.json` the same failure stays on route
  `capture`, `view.phase` `"failed"`, message verbatim, a second later. The no-report
  case is this feature's own `capture-view.json` failure half (stays on capture).

Record it (SKILL.md, Evidence). Each feature has one evidence record per head, so
both scenarios go into **one** `verify_evidence.py record` call, each scenario's
artifacts renamed to a unique prefix first, and `--result` is `fail` if **either**
scenario's exit is not 0 (browser-navigation.md explains why).

```sh
python3 "$H" doctor --instance "$INSTANCE"
mkdir -p "$WT/.verify-run/$INSTANCE"
REC="$(mktemp -d "$WT/.verify-run/$INSTANCE/record-capture-view.XXXXXX")"
RESULT=pass
for name in $SCENARIOS; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature capture-view \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario(s) $SCENARIOS (desktop-gpui/probes/<name>.json) on instance $INSTANCE" \
  --observed "$(for name in $SCENARIOS; do printf '%s: %s; ' "$name" "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$OUT/$name/result.json")"; done)" \
  --side-effect "opened sink stayed [] on the success, failure and permission-route paths; see each scenario's renamed result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
```

`RESULT` above is read straight from `result.json`'s own `exit` field (ruling
R-ab: never hard-code `pass`), so a failing run records `--result fail`
automatically; name the transcript's failing step in `--observed` when it does.

## Gotchas

- `scenario` replaces the instance's process and temp root with a fresh one before
  it runs (SKILL.md's Drive section), so run it before any other proof that depends
  on this instance's prior state, or relaunch after.
- A narrow panel truncates long URIs with an ellipsis; the scenario's text matcher
  (probes/README.md) tolerates this on the URI assertion the full scenario makes.
- This feature is Phase 1 work: today's stub always reports `phase: "stub"` and
  never shows a Capturing/captured/failed sequence, so `scenario --name
  capture-view` exits 1 at its first `view.phase` check until Phase 1 lands.
  `proven: no` stays until then.
- `capture-permission-route` is a Phase 2 parity scenario: on the Phase 1 head
  (5017f5b) it exits 1 at "automation missing: the failed capture routes to
  onboarding" (route stays `capture`) until Phase 2 lands the Tauri `capture-err`
  routing.
