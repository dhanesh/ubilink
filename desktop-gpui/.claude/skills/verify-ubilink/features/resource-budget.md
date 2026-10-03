# resource-budget: The release app idles light and ships small

- id: resource-budget
- proven: no
- anchors: probes/performance.json, src/automation/store.rs:MetricKind, src/automation/store.rs:record_metric

## What it is

With the real Phase 1 views in place, the release build (no automation channel)
idles under 0.5% CPU over 60 seconds, under 80 MB physical footprint, and strips to
under 20 MB — the numbers `docs/PERFORMANCE.md` records. Phase 1 R15 / spec O1: the
GPUI app must stay lighter at idle than the webview shell it replaces, even once it
has real content to render, not just the Phase 0 stub's static title.

## How to reach it

- **Not driven through this skill's automation channel at all.** The release build
  this feature measures carries no automation socket by construction (spec S1); it
  is launched and measured by `scripts/probe.py run performance`'s `release_budget`
  step (`cargo build --release`, no `automation` feature, its own isolated temp
  root), never by `ubilink_instance.py launch`.
- **This skill's role is to read the resulting record**, `docs/PERFORMANCE.md`, and
  to confirm its anchored source (`desktop-gpui/probes/performance.json`) is the
  measurement that produced it.

## Drive it

```sh
cd "$WT"
make -n budget | grep -q "scripts/probe.py budget" && echo "make_target: ok"
python3 scripts/probe.py run performance
echo "exit $?"
```

Expected: `make -n budget`'s dry run prints a recipe line running
`scripts/probe.py budget` (a no-op target fails this check); `probe.py run
performance` exits 0 once the release build's idle CPU, footprint and stripped
binary size, and the automation build's 50-trigger hotkey latency and dismiss
regressions, are all within budget and agree with `docs/PERFORMANCE.md`
(`probes/README.md`'s "PERFORMANCE.md format"). This is the one Phase 1 feature
whose drive is `probe.py run`, not this skill's `$H call` — there is no automation
instance to launch for a release-build measurement, so Launch/Doctor (SKILL.md)
do not apply to it the way they do to every other feature in this map.

## Proof

- **`docs/PERFORMANCE.md` records every required row**, each within its stated
  budget (`idle_cpu_percent < 0.5`, `physical_footprint_mb < 80`,
  `stripped_binary_mb < 20`, plus the two `panel-hotkey.md` rows in the same table).
- **The numbers agree with a fresh measurement**, within the tolerances
  `probes/README.md`'s format table states (`± 0.25` CPU points, `± max(10 MB, 15%)`
  footprint, `± 1 MB` binary size).
- **`make budget` is wired to the real measurement**: `make -n budget` must show a
  recipe line running `scripts/probe.py budget`, so a no-op or stale target fails
  this check even if the doc itself happens to look right.

## Gotchas

- `docs/PERFORMANCE.md` does not exist yet (Phase 1 R15 is what writes it with
  `make budget`), so it cannot be an anchor here (the forge lint requires every
  anchor to resolve against a file that exists now) — the anchors above instead name
  the measurement's source, `probes/performance.json`, and the metrics ring it reads
  (`src/automation/store.rs`), which do exist today.
- A `vmmap` refusal, or a release build that cannot be built `--locked` on this
  host, is ENV-BLOCKED, never a product FAIL (`probes/README.md`'s `release_budget`
  step).
- This feature's numbers are only meaningful once Phase 1's real views replace the
  stubs: a budget run against the Phase 0 app under-measures what the shipped app
  will actually cost. `proven: no` stays until Phase 1's views are in and `probe.py
  run performance` is driven live.
