# resource-budget: The release app idles light and ships small

- id: resource-budget
- proven: no
- anchors: probes/performance.json, src/automation/store.rs:MetricKind, src/automation/store.rs:record_metric, src/views/capture.rs, src/views/browser.rs, src/views/onboarding.rs, src/views/settings.rs

## What it is

With the real Phase 1 views in place, the release build (no automation channel)
idles under 0.5% CPU over 60 seconds, under 80 MB physical footprint, and strips to
under 20 MB — the numbers `docs/PERFORMANCE.md` records. Phase 1 R15 / spec O1: the
GPUI app must stay lighter at idle than the webview shell it replaces, even once it
has real content to render, not just the Phase 0 stub's static title.

## How to reach it

- **Not driven through this skill's automation channel at all.** The release build
  this feature measures carries no automation socket by construction (spec S1); the
  `scenario` command below builds and launches it as a second, isolated instance
  inside the scenario's own `release_budget` step, never through
  `ubilink_instance.py launch`.
- **This skill's role is to drive the measurement and read the resulting record**,
  `docs/PERFORMANCE.md`, that the scenario's `perf_doc` steps check.

## Drive it

The acceptance proof is the frozen scenario `probes/performance.json`, run through
the harness's `scenario` command. It is the one feature in this map whose scenario
measures a release build the harness itself did not launch; `scenario` builds and
runs it inside the `release_budget` step, then checks `docs/PERFORMANCE.md` against
this run's own measurements.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name performance
echo "exit $?"
```

Expected: exit 0. `$OUT/performance/result.json`'s `verdict` is `"PASS"`;
`transcript.txt` shows PASS for "docs/PERFORMANCE.md records every budget row, each
within budget", "release app: idle CPU < 0.5 % over 60 s, footprint < 80 MB,
stripped binary < 20 MB", and "PERFORMANCE.md agrees with this run, and `make
budget` runs probe.py budget". `result.json`'s `saved` array holds this run's own
`idle_cpu_percent`, `physical_footprint_mb` and `stripped_binary_mb` (from the
`release_budget` step) for comparison against the doc.

## Proof

- **`docs/PERFORMANCE.md` records every required row**, each within its stated
  budget (`idle_cpu_percent < 0.5`, `physical_footprint_mb < 80`,
  `stripped_binary_mb < 20`, plus the two `panel-hotkey.md` rows written by the same
  scenario run).
- **This run's own measurement is within budget and agrees with the doc**, within
  the tolerances `probes/README.md`'s format table states (`± 0.25` CPU points,
  `± max(10 MB, 15%)` footprint, `± 1 MB` binary size) — the scenario's final
  `perf_doc` step with `compare: true` checks this directly against `result.json`'s
  `saved` values.
- **`make budget` is wired to the real measurement**: the same step's
  `make_target: "budget"` requires `make -n budget` to show a recipe line running
  `scripts/probe.py budget`, so a no-op or stale target fails this check even if the
  doc itself happens to look right.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/performance"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature resource-budget \
  --verifier "$VERIFIER" --result pass \
  --action "scenario performance (desktop-gpui/probes/performance.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"], r["saved"])' "$R/result.json")" \
  --side-effect "docs/PERFORMANCE.md agrees with this run's own idle_cpu_percent/physical_footprint_mb/stripped_binary_mb; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- `docs/PERFORMANCE.md` does not exist yet (Phase 1 R15 is what writes it, via
  `make budget` running `scripts/probe.py budget`), so it cannot be an anchor here
  (the forge lint requires every anchor to resolve against a file that exists now)
  — the anchors above instead name the measurement's source
  (`probes/performance.json`), the metrics ring it reads
  (`src/automation/store.rs`), and the four views whose rendering cost the idle
  CPU/footprint numbers actually measure once Phase 1 lands.
- `performance.json` also contains the identical 50-trigger hotkey-latency loop and
  dismiss-regression steps that `panel-hotkey.md` and `panel-dismiss.md` each run on
  their own (`probes/panel-hotkey.json`, `probes/panel-dismiss.json`), copied here at
  Phase 0 because Phase 1 may change neither file (spec T4). Running this feature's
  scenario alone re-exercises those same assertions; it is not a substitute for
  recording `panel-hotkey`/`panel-dismiss` as their own feature proofs, since this
  scenario additionally requires the release-build measurement that theirs do not.
- A `vmmap` refusal, or a release build that cannot be built `--locked` on this
  host, is ENV-BLOCKED (exit 3), never a product FAIL.
- This feature's numbers are only meaningful once Phase 1's real views replace the
  stubs: a budget run against the Phase 0 app under-measures what the shipped app
  will actually cost. `proven: no` stays until Phase 1's views are in and `scenario
  --name performance` is driven live.
