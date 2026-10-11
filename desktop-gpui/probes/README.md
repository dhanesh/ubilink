# Probe scenarios

Each `<name>.json` here is a scenario that `scripts/probe.py run <name>` runs against
fresh, isolated automation instances of `ubilink` (GPUI Phase 0 R8). The Phase 1
spec's `[proof:]` clauses become scenarios in this format, so the format is part of
the contract. Change it only together with `scripts/probe.py`, and keep
`scripts/test_probe.py` passing.

```
python3 scripts/probe.py run shell                  # one instance
python3 scripts/probe.py run shell --instances 2    # two at once, plus isolation checks
python3 scripts/probe.py run shell panel-hotkey     # several scenarios in one run
python3 scripts/probe.py list --expect 15           # the Phase 1 scenarios, each run, with results
python3 scripts/probe.py budget --write docs/PERFORMANCE.md  # measure R15's numbers into the doc
python3 -m unittest scripts/test_probe.py           # the engine's self-tests (no app)
```

## What a run does

1. Builds the automation binary once with
   `cargo build --locked --message-format=json-render-diagnostics --features automation --manifest-path desktop-gpui/Cargo.toml`
   using the shared warm target: an exported `CARGO_TARGET_DIR` if set (the factory
   conductor exports one for every clone, ruling R-ad), else
   `CARGO_TARGET_DIR=~/Library/Caches/ubilink-probe/target`. The first time
   that directory is empty, it is seeded from `desktop-gpui/target` with `cp -c -R`,
   an APFS clone that costs seconds and no disk space. Later builds, including ones
   from fresh git clones, only rebuild the path crates.

   That target dir is shared by every clone and commit, so the probe holds an
   exclusive flock on `~/Library/Caches/ubilink-probe/target.lock` while it builds
   **and** while it clones the produced executable (the path cargo reports in its
   `compiler-artifact` message, not an assumed one) into a per-run directory. The
   instances run that copy. Another probe rebuilding the shared target for a
   different commit cannot swap the binary under this run. The run prints the
   copy's sha256 and the source commit (`git rev-parse HEAD`, plus `+dirty` if
   `desktop-gpui/`, `app/`, `core/` or `design/` have uncommitted changes) at
   the start (`probe: automation binary sha256=... commit=...`) and again on the
   last line (`ran: ...`), so the evidence names exactly what ran.
2. Compiles the Swift helpers (`scripts/probe-helpers/quartz.swift`, plus
   `scripts/ocr.swift` if a scenario has an `ocr` step) with `swiftc -O` into
   `~/Library/Caches/ubilink-probe/bin/`. Each binary is named by its source hash.
   The OCR helper is warmed once after compiling, because Vision's first run of a
   new binary compiles its model for about a minute.
3. Takes the **desktop lock**, an exclusive `flock` on
   `~/Library/Caches/ubilink-probe/desktop.lock` (spec O4). The pasteboard, the key
   window and the cursor are shared by the whole desktop, so two probes never run
   at once. A second probe waits and prints who holds the lock, but every lock wait
   (this one and the target lock) ends 480 s after the probe started: it then exits 3,
   ENV-BLOCKED "desktop busy" (or "build cache busy"), inside the factory verifier's
   600 s per command (final review I8). GUI proofs serialise on one desktop; parallel
   tasks overlap only their builds.
4. For each scenario, launches N instances. Each one gets its own temp root under
   `$TMPDIR` (`ubprobe-<instance>-*`) and runs the binary directly, not through
   `open`, with:
   - `UBILINK_INSTANCE=p<pid>-<scenario>[-k]`, so it has its own automation socket at
     `$TMPDIR/ubilink-<uid>/<instance>/auto.sock`
   - `UBILINK_INSTANCE_SOCKET=<root>/si.sock`, so it has its own single-instance
     socket and never signals your running app or a sibling
   - `UBILINK_DATA_DIR=<root>/data`, which desktop-gpui's one data-dir resolver
     (`src/paths.rs`, `data_dir()`) honours in every build. `state` reports the
     resolved path as `data_dir`. An automation build started with
     `UBILINK_INSTANCE` but without `UBILINK_DATA_DIR` refuses to start (exit 2), so
     a misconfigured harness can never touch your real data.
   - `UBILINK_SKIP_HOTKEYS=1`, so no real global hotkeys are registered (O4)
   - a private copy of `probes/fixtures/` at `<root>/fixtures`

   Any `UBILINK_*` variable exported in your shell is dropped first. Each root gets
   an `owner.json` (owning probe pid, instance pid, binary) at launch, and `run`
   first reaps orphans (final review I7): an instance whose owning probe is dead
   (SIGKILLed by a verifier timeout, so its teardown never ran) is terminated, after
   checking its pid still runs the recorded binary, and its root removed. The verify
   harness's instances outlive their commands by design; they are reaped only when no
   harness claim accounts for them.
5. Runs the steps, then stops every instance (SIGTERM, then SIGKILL after 5 s) and
   removes its socket directory and temp root. This also happens on a failure,
   Ctrl-C or SIGTERM.

Opens never launch anything. In an automation build the app's opener records them
in the `opened` sink. File dialogs take fixture paths (`load_fixture`).

## File format

```json
{
  "description": "one line: what this proves",
  "requirement": "R4",
  "notes": ["optional free text"],
  "setup": {
    "files":    {"{data_dir}/plugins-state.json": "some-state.json"},
    "graph":    "graph-backlinks.json",
    "fixtures": {"capture": "capture-delayed.json"}
  },
  "steps": [
    {"call": "trigger_hotkey", "args": {"action": "capture"}},
    {"call": "state", "retry_ms": 3000, "expect": {"window_verified": true}},
    {"see": {"role": "Label", "text": "Capturing"}}
  ]
}
```

- `setup.files` copies files into the instance before launch. Each key is a
  destination (placeholders allowed) and each value is a source relative to the
  instance's fixture copy, or an absolute path.
- `setup.graph` (a path or a list of them, relative to the fixture copy) seeds the
  instance's `{data_dir}/ubilink-graph.sqlite` from `ubilink.graph.v3` fixtures
  before launch, the same way the `seed_graph` step does (below).
- `setup.fixtures` sends `load_fixture {kind, path}` after launch. Relative paths
  resolve against the instance's fixture copy. The kinds are `capture`,
  `permission`, `graph_dir`, `export_path`, `import_path`, `login_item` and `accent`
  (`desktop-gpui/src/automation/protocol.rs`).
- Unknown top-level keys, unknown step types and unknown step options are
  **scenario errors** (exit 2). A typo can never make a step pass.

### Placeholders

Strings in `args`, `expect`, paths and `setup` may use `{root}`, `{data_dir}`,
`{fixtures}`, `{probes}` (this directory), `{instance}` (the `UBILINK_INSTANCE`
value), `{repo}` (the repository root; the Phase 1 scenarios use it only for
`docs/PERFORMANCE.md`, R15's output) and, after a `pasteboard: set_nonce`, `{nonce}`.

### Steps

Each step is an object with exactly one step key. Any step may also have a `name`,
which is shown in the report, and a `block` id: consecutive steps with the same
`block` are one iteration, the unit a focus-theft retry re-runs (see
[Unattended guards](#unattended-guards)).

| Step | Value | Options | Checks |
|---|---|---|---|
| `call` | verb name | `args`, `expect`, `expect_error`, `retry_ms`, `save` | Sends `{"verb", "args"}` and requires `ok`, then every `expect` entry against `result`. `expect_error` (substring, or `true`) requires `ok: false` instead. `retry_ms` re-sends until the step passes or the time runs out. It is allowed only for read-only verbs: `state`, `metrics`, `window_id`, `a11y`, `opened`. `save: {name: "dot.path"}` records values from the result (a missing path fails the step) for a later `{"saved": name}` matcher or `perf_doc compare`. |
| `see` | a query, or a non-empty list of them (see [`see` queries](#see-queries)) | `absent`, `exact`, `count`, `min_count`, `max_count`, `route`, `timeout_ms` (default 3000) | Polls `a11y` (GPUI's AccessKit tree, read in-process with no TCC grant, spec TN5) until every query holds. `route` first requires `state.route` to equal it. `absent` also requires the tree to have at least one node other than the Window, so an empty tree can't pass it. Roles compare case-insensitively, ignoring punctuation. Text matching is described below. |
| `ocr` | text or list | `absent`, `exact` | Runs `screencapture -l<window_id>` on the panel, then Vision OCR (`scripts/ocr.swift`), and requires each text in some line (or in all lines joined). Needs Screen Recording. |
| `pixel` | `{x, y, w?, h?}` in window points from the top-left, or a dot path into `state` where the view reports a region `{x, y, w, h}` of at least 24 × 24 pt (smaller is a FAIL: a real surface, not a swatch) | `color` (`#rrggbb`, required), `delta_e` (default 3.0), `match` (`mean`, default, or `best`) | The region's mean sRGB colour (`best`: its closest single pixel, for a text run whose antialiased edges are not the text colour) must be within CIE76 ΔE of `color`. A state path that holds no region is a FAIL (the view did not report it). Uses the same capture as `ocr`, so it also needs Screen Recording. |
| `pasteboard` | `"set_nonce"` or `"read"` | `expect`, `changed` | `set_nonce` puts a unique string on the pasteboard and records the change count. `read` compares `pbpaste` against `expect` (a literal, which may be `"{nonce}"`, or a matcher). `changed: true/false` requires the change count to have risen (or not) since the last pasteboard step. |
| `graph` | `true` | `db`, `export`, `links`, `no_links`, `link_count`, `resource_count`, `timeout_ms` | Reads the instance's graph read-only. The default is `{data_dir}/ubilink-graph.sqlite` (ubilink-core's file). `db` names another file, and `export` reads a Settings > Export JSON instead. `links`/`no_links` are `[[uriA, uriB], ...]` unordered pairs. |
| `file` | path | `exists` (default true), `expect`, `timeout_ms` | The file exists (or not). With `expect`, it parses as JSON and meets the path expectations. |
| `winframe` | `"any"`, `"cursor"`, `"state"`, `"gone"` or a display index | `timeout_ms` (default 2000) | Finds the panel's frame through Quartz `CGWindowListCopyWindowInfo`, matched by `window_id`. It must lie within any display, the display under the cursor, the display `state.shell.display_id` names, or display *n*. `"gone"` instead requires the window found by the most recent `winframe` (or `window_id` call) for the current panel to be absent from Quartz's on-screen list (a hide checked at the window server, not just in `state`). A call showing a new panel (hidden → visible) forgets the old number, and a passing `gone` uses it up, so with no such window it fails. |
| `warp_cursor` | display index, or `{display, x, y}` (fractions, default 0.5) | | Moves the cursor with `CGWarpMouseCursorPosition`. No event is posted and no Accessibility grant is needed. |
| `activate_other` | `true` | `settle_ms` (default 300) | A real loss of key status for the panel: launches the probe's own activator (`scripts/probe-helpers/activator.swift`, compiled like the other helpers), a minimal app that makes itself active with a 1-point key window, waits for it to report `active`, holds for `settle_ms`, then quits it. No user app is ever launched (O4), and none is left running. A real deactivation test necessarily takes focus from your app for that moment. If the activator cannot become active in 8 s the step is ENV-BLOCKED. |
| `restart` | `true` | | Stops the instance and relaunches it with the same temp root (data survives), then replays every fixture loaded so far, since fixtures live in app memory. |
| `sleep_ms` | milliseconds | | Waits. Prefer `retry_ms` or `timeout_ms`. |
| `key_until` | a node query (as in `see`) | `key` (default `tab`), `max_presses` (default 40, at most 200), `route` | Checks the query, then presses `key` (the `key` verb) and checks again, until it holds; FAIL after `max_presses`. "This control is reachable from the keyboard" (Phase 1 U4) without the probe hard-coding a view's tab order: the target is usually `{"role": "Button", "text": "Export", "state": {"focused": true}}`. |
| `seed_graph` | a `ubilink.graph.v3` fixture path | `replace` (default false) | Writes the fixture into the instance's `{data_dir}/ubilink-graph.sqlite` with ubilink-core's own schema and the same statements `LinkGraph::import_json` runs (upsert by URI, guids kept, `updated_at = created_at`, resources in file order, one row per unordered link, dangling and self links skipped); `replace` first deletes every link and resource like `LinkGraph::clear`. Merges into a live instance's graph (SQLite locking makes it one more writer). It skips only the core's URI validation, so fixtures use allowed schemes. `test_probe.py` pins the schema to `core/src/graph.rs`. |
| `process` | `"unchanged"` | | The instance is still the process this scenario launched (or last restarted), and alive: "the same pid" (Phase 1 R14). |
| `screenshot` | a file-name-safe label | | Captures the panel like `ocr` and keeps the PNG at `~/Library/Caches/ubilink-probe/shots/<run>/<scenario>-<label>.png` (instance temp roots are deleted; the run prints the directory). Needs Screen Recording. |
| `release_budget` | `true` | `cpu_pct_lt`, `footprint_mb_lt`, `binary_mb_lt` (all required), `idle_s` (default 60), `settle_s` (default 5) | Builds the stripped release build (no automation channel) and launches it as a second isolated instance (see [Release-build instances](#release-build-instances)), waits `settle_s`, then measures idle CPU as the `ps -o time=` CPU-time delta over `idle_s` seconds of wall time, physical footprint with `vmmap --summary`'s `Physical footprint:` line (Activity Monitor's Memory number) at the end, and the binary's size after `strip` (a copy). Each must be under its budget. Saves `idle_cpu_percent`, `physical_footprint_mb`, `stripped_binary_mb`. A `vmmap` refusal is ENV-BLOCKED. |
| `perf_doc` | path of `docs/PERFORMANCE.md` (use `{repo}`) | `compare` (default false), `make_target` | Parses the doc's budget table ([PERFORMANCE.md format](#performancemd-format)): every row present once, each budget cell exactly as specified, each measured value under its budget. `compare` also requires each value to agree with this run's saved measurement within the row's tolerance. `make_target` requires `make -n <target>` to exit 0 at the repo root and print a recipe line running `scripts/probe.py budget` (a dry run: make prints the recipe but runs only `$(shell ...)` and `+` lines). |

### `see` queries

The tree is flattened in **document order**: a depth-first walk from the dump's
`root` through `children`, the order the view laid its nodes out. Each node has a
role, its texts (`label`, `value`, `description`, `placeholder`, `tooltip`), and its
state, taken from what gpui-pre 0.3.7's `debug_a11y_tree_json` emits:

| State key | From the dump |
|---|---|
| `selected`, `expanded` | booleans |
| `toggled` | `"True"`/`"False"`/`"Mixed"`, normalised to `true`/`false`/`"mixed"` |
| `focused` | derived: the node is the dump's `gpui_focus` or `active_descendant_focus` |
| `actions` | the `on_action` list (`"Click"`, `"Focus"`, ...). Not text. |
| `orientation`, `numeric_value`, `min_numeric_value`, `max_numeric_value`, `numeric_value_step`, `level`, `position_in_set`, `size_of_set`, `row_index`, `column_index`, `row_count`, `column_count` | as emitted |

There is no `disabled` flag in 0.3.7's dump, so a `disabled` expectation is refused.
A state the node doesn't report counts as `null`: `{"selected": false}` does not
match a node that isn't selectable, while `{"selected": {"ne": true}}` does. State
values are literals or matchers, as in `expect`.

Query shapes:

- **Node query** `{"role"?, "text"?, "state"?}` (role or text required): some node
  matches all of them. With `absent`, no node does. `count`, `min_count` and
  `max_count` bound how many match.
  `{"role": "ListItem", "state": {"selected": true}, "text": "beta.md"}`
- **Index query** `{"role", "nth", "text"?, "state"?}`: the nth node of that role (0-based,
  negative counts from the end) exists and matches. "Row 3 carries the selected
  flag" is `{"role": "ListItem", "nth": 2, "state": {"selected": true}}`.
- **Sequence query** `{"sequence": {"role", "texts": [...], "mode"}}`: the texts of that
  role's nodes, in document order. `prefix` (default): the first nodes match `texts`
  in order ("top 3 rows equal the expected top 3"). `exact`: there are exactly that
  many nodes, matching in order ("exactly the 3 backlinks, newest first"). `ordered`:
  the texts appear in that relative order, with other nodes allowed between them.

- **Alternative** `{"any": [[query, ...], [query, ...]]}`: holds when every query of at
  least one alternative holds (at least two alternatives, not nested). Phase 1 R13's
  "shows enabled, OR requires-approval with an explanation" (D6).

Node, index and sequence queries may add `"deep": true` (in a sequence, inside
`sequence`): the text then matches any ONE text in the node's subtree, not only the
node's own. gpui-component's `List` puts a row's visible text in `Label` children of
the `ListItem` node (A11Y-NOTES.md), so "row 2 shows this URI" is
`{"role": "ListItem", "nth": 1, "text": "<uri>", "deep": true}`. Texts are never
concatenated, so two labels cannot combine into a match neither holds, and a truncated
label still matches on its own. A query may also carry its own `"exact"`, which
overrides the step's `exact` for that query.

`absent` and the count options apply only to node queries.

### No vacuous checks

A check that inspects nothing must not pass, so validation refuses:

- empty or whitespace `text` (`see`, `ocr`, the `text` operator) and empty
  `contains`/`not_contains` operands
- empty query lists (`see: []`, `ocr: []`, `ocr: ""`)
- an empty `state` object, and empty or malformed `links`/`no_links` pairs
- a `pasteboard: read` with neither `expect` nor `changed`

`text_matches` itself also never matches empty text.

Displays are numbered with the main display as 0 and the rest from left to right.
If a step names a display the host lacks, the result is ENV-BLOCKED, not FAIL.

### Expectations

`expect` maps a dot path into the result (`shell.window_kind`, `view.rows.0.uri`, or
`""` for the whole result) to either:

- a **literal**, compared for equality. A missing key never equals anything, not
  even `null`.
- a **matcher**, an object whose keys are all operators: `eq`, `ne`, `in`,
  `contains`, `not_contains`, `text` (the normalised matching below), `len`,
  `min_len`, `max_len`, `lt`, `le`, `gt`, `ge`, `type` (`list`, `object`, `string`,
  `number`, `bool`, `null`), `matches` (regex search) and `exists`. All operators in
  one object must hold.

In a `call` step's `expect`, the operand of `eq`, `ne`, `lt`, `le`, `gt` or `ge` may be
`{"path": "<dot path>"}`: the value at that path in the **same** result, read when the
check runs. This compares two live measurements in one state read, for example
`"view.layout.last_adapter.bottom": {"le": {"path": "view.layout.viewport.h"}}`. A
path that is missing never passes, and a failure prints both values. Anywhere else
(`see` state flags, `pasteboard`, `file`) a path operand is a scenario error.

### Text matching (`see`, `ocr`, the `text` operator)

Both sides are NFKC-normalised (this turns `…` into `...`), casefolded and
whitespace-collapsed. The expected text must then be a substring of the node's text,
or equal to it with `exact`. A narrow panel truncates by design, so a node ending in
an ellipsis also matches when the visible text before the ellipsis ends with at
least max(12, half the expected text's length) characters (or all) of the start of
the expected text. For example, `"Capture: https://example.c…"` matches
`https://example.com/page` (needs 12), while `"https://…"` matches no URI: an
over-truncated row is no evidence for a sequence, and cannot hide a decoy from an
`absent` check either. A leading ellipsis
works the same way from the other end. A node's text is every string in its `aria`
object except the role (value, label, placeholder, description).

## Unattended guards

Phase 1 runs probes with nobody watching (R15), so the runner keeps environment
trouble from passing for a product failure:

- **Display awake.** `run` holds `caffeinate -d -i -u` for the whole run (tied to the
  probe's pid, terminated in teardown), which also wakes a display that was asleep.
  If the display still reports no active display, any step that needs one is
  ENV-BLOCKED with "display asleep", and a nil window list from the window server is
  ENV-BLOCKED too, never an empty list.
- **Focus theft.** A capture or browser panel hides on any loss of key status, by
  design, so another app activating mid-step can hide it. When a step that asserts
  visibility fails (`winframe`, `window_id`, `resign_key`, or a `call` whose `expect`
  names `visible`, `route` or `closed`) and the frontmost app changed during the step
  to one that is not the probe, its instance or its activator, the iteration (its
  `block`, or the step alone) is re-run once from a closed panel (`RETRY` in the
  report). A second theft makes it ENV-BLOCKED ("focus taken by <app>"). A failure
  with no theft is a FAIL as usual.

## Report and exit codes

The report has one line per step: `PASS`, `FAIL`, `ENV-BLOCKED` or `ERROR`. A
failure also prints the expected and actual values. The first FAIL stops that
instance's steps. The first ENV-BLOCKED step does too: every later step of that
instance is reported `SKIPPED` (not run, counted as "skipped after env-blocked"),
because the GUI state it builds on is unknown, so a later failure would be the
environment's, not the product's (final review minor T8). Other scenarios in the run
still run.

| Exit | Meaning |
|---|---|
| 0 | every step passed |
| 1 | a product FAIL |
| 2 | a scenario error (bad file, unknown step or option) or a harness error (build failure, an instance that does not start, a socket timeout or close, a helper failing): never counted as the product failing |
| 3 | nothing failed, but some step was ENV-BLOCKED (missing Screen Recording, too few displays, display asleep, focus taken twice, no swiftc) |

With `--instances N` (N ≥ 2), every instance runs all the steps. While all of them
are still running, the probe then checks that each one answers `state` with its own
`data_dir`, that each `opened` sink answers, and that pids, sockets, data
dirs and fixture dirs are all distinct (R8, O1).

## Headless tests (`cargo-test`)

`python3 scripts/probe.py cargo-test <name> [--features ""]` is how Phase 1's
headless criteria run one named desktop-gpui test (final review I4). It lists the
crate's tests (`cargo test --locked --features automation -- --list`, the default
features), selects those whose full path or last `::` segment is exactly `<name>`
(never a substring), runs only those with `--exact`, and exits 0 only when at least one
ran and every one reported `ok`. A missing, failed or ignored test exits 1; a build
failure exits 2; the shared build cache staying busy exits 3. Plain `cargo test
<filter>` exits 0 when nothing matches, which is why the criteria do not use it. It
builds in the same shared target, under the same target lock, as `run`.

## Release-build instances

`Instance(label, build, release=True)` (Python API, used with
`build_binary(run_dir, release=True)`) launches the stripped release build as
shipped, for probes that measure the real app (Phase 1 R15). It gets the same
isolation: its own temp root, `UBILINK_DATA_DIR`, single-instance socket and
`UBILINK_SKIP_HOTKEYS=1`. It has no automation channel, so it counts as up once its
single-instance socket exists, and `call()` refuses. The `release_budget` step uses it
(Phase 1 R15, `performance.json`); `run` builds the release binary before taking the
desktop lock when a scenario has that step.

## Limits

- `pasteboard: set_nonce` saves your pasteboard **text** first and restores it at the
  end of the run. Non-text contents, such as images or files, are not restored.
- `ocr` and `pixel` need Screen Recording for the process running `probe.py`.
  Without it they are ENV-BLOCKED. Text proofs should use `see`.
- `warp_cursor` and `activate_other` act on the real desktop. That is why every run
  holds the desktop lock. `activate_other` briefly takes focus from whatever you are
  using.

## Phase 1 acceptance scenarios (`list`)

The 15 scenarios that judge GPUI Phase 1's unattended work are written here, at
Phase 0, because Phase 1 may change neither this directory nor `scripts/probe.py`
(spec T4). `phase1.json` names them and their requirement:

| Scenario | Req | Scenario | Req | Scenario | Req |
|---|---|---|---|---|---|
| `capture-view` | R1 | `link-browser-filter` | R5 | `settings-adapters` | R10 |
| `copy-link` | R2 | `browser-select` | R6 | `graph-roundtrip` | R11 |
| `backlinks` | R3 | `browser-drill` | R7 | `graph-reset` | R12 |
| `link-browser` | R4 | `link-marking` | R8 | `launch-at-login` | R13 |
| | | `onboarding` | R9 | `theming` | R14 |
| | | | | `performance` | R15 |

`accent.json` (D26, the live system accent) is not one of Phase 1's fifteen; it runs
by name like the parity scenarios.

```
python3 scripts/probe.py list                                   # run all 15, print name / req / result
python3 scripts/probe.py list --expect 15 --expect-failing 15   # Phase 0: all 15 must be product FAILs
python3 scripts/probe.py list --no-run --expect 15              # validate only
```

`list` validates each scenario (its `requirement` must start with the manifest's id),
runs each in its own `probe.py run <name>` (so each exit code is exactly what that
command reports; full output goes to `~/Library/Caches/ubilink-probe/list-logs/<run>/`),
and prints a table with the first failing step. `--expect N` requires N scenarios.
`--expect-failing N` requires exactly N to exit 1 (product FAIL) and the rest 0: an
exit 2 (scenario error) or 3 (ENV-BLOCKED) says nothing about the product, so it
counts against the expectation. Against the Phase 0 stub views every one fails at a
view-specific step (its first a11y, view-state or PERFORMANCE.md check).

Phase 1's guard is `git diff --exit-code <phase0-sha> -- desktop-gpui/probes
scripts/probe.py ...`. Every input of these scenarios lives under
`desktop-gpui/probes/` or `scripts/` (fixtures, Swift helpers), except
`docs/PERFORMANCE.md`, which is R15's product output under judgment, not an input.
Expected colours, URIs, row orders and the golden table are written into the
scenario files, never read from `design/`, `core/` or `app/` at run time.

## Phase 2 parity acceptance scenarios

Four scenarios judge GPUI Phase 2's Tauri-parity work. They were written against the
Phase 1 head (5017f5b), where each one exits 1 at the step named below, after every
earlier step passes. They are not in `phase1.json`; run them by name.

| Scenario | Requirement | Fails on the Phase 1 head at |
|---|---|---|
| `capture-permission-route` | A failed capture while the `permission` report says Automation is not granted switches the same window (same `window_id`, same process) to route `onboarding` with its steps listed and nothing opened. With Automation granted, whether everything is granted or only Accessibility is missing (`fixtures/permissions-accessibility-missing.json`), the failure stays on route `capture`: `view.phase` `failed`, message verbatim, checked a second later. With no report (`capture-view.json`'s failure half) it also stays. Since rc.4 the open check (`capture-permission-gate`) reaches onboarding first in round 1; the failure route itself is proven by a same-instance round where the all-granted report confirmed the session, the report then says not granted (revoked) and the next failed capture routes to onboarding. Tauri `dist/panel.js` `capture-err`. | "automation missing: the failed capture routes to onboarding" (route stays `capture`) |
| `copy-dismiss` | After Enter (then Cmd-C on a restarted instance) copies, the Copied Label shows and the panel stays visible with it about 1 s after the key. The panel then hides by itself within about 3 s of the key: `shell.visible` false, route null, `window_id` refuses, `winframe: gone`. The pasteboard holds the URI. | "Enter: the panel hides by itself within ~3 s of the key" |
| `browser-left-close` | While drilled in, Left goes back and the panel stays. At the top level with a non-empty filter, Left only edits the filter: three Lefts take the caret past its start edge, `x` then types at the start and the panel stays. At the top level with an empty filter, Left closes the panel and opens nothing. Tauri `dist/panel.js` 567-569, `browseBack`. | "empty filter: Left at the top level closes the panel" |
| `backlink-open` | In the capture view with backlinks, Enter with no row focused copies and opens nothing. On a restarted instance, Tab moves keyboard focus onto a backlink row: the row's own `ListItem` node carries `focused`, and exactly one row does. Enter then opens exactly that row's URI through the core opener (`opened` length 1) and does not copy (the pasteboard keeps its nonce and its change count). | "open: Tab moves focus onto the Beta Post backlink row" (Tab focuses the Copy link button) |

Phase 2 view contract additions these imply, beyond "Phase 1 view-state contract" below:

- **Capture → onboarding.** On a failed capture, the capture path reads the same
  permission report onboarding reads (`crate::fixtures::path(Fixture::Permission)` in
  automation builds). It switches the panel to the onboarding route in place only
  when that report exists and its `automation` is not `"granted"`.
- **Copy self-dismiss.** After a copy, keep `Copied` visible at least 1 s, then hide
  the panel the way a dismiss does (about 1.2-1.5 s is the intended window; the
  probe allows up to about 3 s from the key).
- **Browser Left.** `MoveLeft` at the top level closes the panel only when the filter
  text is empty, wherever the caret is.
- **Backlink rows are focusable.** Each backlink `ListItem` takes keyboard focus with
  Tab (`focused` = `gpui_focus` or `active_descendant_focus` on that node), and Enter
  on a focused row opens its URI through the shell's opener instead of copying.

## Phase 2b parity regression scenarios

A hands-on check of the shipped GPUI app found regressions that the scenarios above
missed: they read the accessibility tree, which has no bounds or text sizes, and
nothing pressed Escape. Five scenarios cover them. The four layout scenarios read
measured layout (`state.view.layout`, below) and compare two measurements with
`{"path": ...}` operands ([Expectations](#expectations)). Before its fix, each must
exit 1 at the step named below, after every earlier step passes. `escape-dismiss` is
checked against 4c6e66d. The layout scenarios fail on the commits
that add the measurements (8cdc431, f2f8176), because 4c6e66d reports no layout to
compare. Run them by name.

| Scenario | Requirement | Must fail before its fix at |
|---|---|---|
| `escape-dismiss` | Escape, a real keystroke through GPUI's keymap. Capture and onboarding hide the panel. The browser cancels a pending mark, then clears a non-empty filter (rows unfiltered again), then hides. Settings closes an open reset question (nothing deleted), then hides. Each hide is confirmed three ways: `shell.visible` false, `window_id` refuses, and `winframe: gone`. Tauri `dist/panel.js` 555-592. | "capture: Escape dismisses the panel" |
| `settings-fit` | With the 10 adapters of `fixtures/adapters.json` (3 rows of 4), the last adapter switch ends inside the grid's scroll box, and the box ends inside the window. This holds when shown, while the reset question adds its row, and after Escape closes it. The window stays on its display (`winframe: state`). Tauri `resize_panel` / `fitWindow`. | "shown: the last adapter switch is fully visible (inside its scroll box, inside the window)" |
| `settings-buttons` | Import, Reset…, Delete everything and Cancel are as tall as Export (measured bounds). Tauri `.set-btn`. | "Import is as tall as Export" (expected 24 vs 28 pt) |
| `settings-login-text` | The launch-at-login status (`Not registered`, then `Enabled` after Space) resolves to the same text size as the `Launch at login` label: 12 pt, `font_size::META`. | "not registered: the status is the label's size" (expected 14 vs 11.38 pt); the check repeats once the status reads Enabled |
| `capture-fit` | With 3 backlinks, and with 4 after `graph-backlinks-newer.json` is merged, the last backlink row ends inside the list's scroll box, and the box ends inside the window. | "3 backlinks: the last row is fully visible" |

What no scenario proves: Settings' **Import** dialog appearing in front of the panel,
and the panel taking keys again after the dialog closes. Automation builds never open
a host file dialog (O4). The Export reveal in Finder (`open -R`) is the same: it runs
only in a release build, so a probe cannot see it. Both are checked by hand
(`docs/RUNBOOK-gui-panel.md`). A probe covers the export write itself through the
`export_path` fixture (`graph-roundtrip`).

Phase 2b view contract additions:

- **Escape** (`views::escape`, one pure decision per route): Settings closes the
  reset question, else dismisses. The browser unmarks, else clears the filter, else
  dismisses. Capture and onboarding dismiss. Dismiss is `shell::Command::Hide`, run
  once the key's dispatch is over.
- **Panel height per route** (`views::RouteEntry::height`). Capture, browser and
  onboarding open at 140 pt. Settings opens at its content height
  (`settings::content_height`). Capture refits to its backlinks when the capture lands
  (`capture::content_height`, at most 8 rows). Settings refits when the reset question
  opens or closes, and when the launch-at-login explanation (or error) line appears or
  goes (Settings reserves up to three 16 pt lines for it). The shell clamps every height to 96..900, keeps the panel's top,
  cuts the height to the display's usable area, and moves the panel up rather than off
  the bottom (`placement::fit_on`). A route switch resizes the same window in place;
  it is never reopened. The width stays 640.
- **Layout** (`state.view.layout`, settings and capture; automation builds only): see
  the table below. What each value measures, and its limits:
  - A button's height is its wrapper's: a `div` whose only child is the button,
    with no padding, so the two heights are equal. gpui-component's `Button` puts
    its children in an inner content box, so a marker inside it would measure that
    box, not the button.
  - A text size is the size GPUI resolves on the element holding the text (the
    label's `div`, the status's `div` around its `text!`). The marker reads the text
    style that element's children inherit. It is not read from rendered glyphs.

## rc.4 permission scenarios

Settings lost the Tauri app's Permissions section in the GPUI port, and capture no
longer led to onboarding until Automation was granted. Two scenarios cover it, and
`settings-buttons` also checks the new button's height. Each exits 1 on 364474e (main
plus these scenarios, before the fix) at the step named; `settings-buttons` there
fails at "Open permission setup… is as tall as Export". Run them by name.

| Scenario | Requirement | Fails before the fix at |
|---|---|---|
| `settings-permissions` | Settings' first section, `Permissions`, has Labels `Automation`, `Read the active app's link.` and a status exactly `Not allowed` (two-missing fixture) or `Allowed` (all-granted), above `App adapters`. `Open permission setup…` is reached with Tab; Enter switches the same window (same `window_id` and process) to onboarding and opens nothing. Tauri `dist/index.html` `set-auto` / `set-open-perms`. | "not allowed: the Permissions section names Automation and its purpose" |
| `capture-permission-gate` | With a succeeding capture fixture and Automation not granted, the hotkey and the tray's Open on capture both switch the same window to onboarding and store nothing (graph `resource_count` 0). With Automation granted the capture lands and stays on route `capture`. After a check said granted, a not-granted report no longer routes (the session is confirmed). Tauri `dist/panel.js` `refreshPermissions`. | "automation missing: capture open routes to onboarding" |

rc.4 view contract additions:

- **Capture open → onboarding.** Until a permission check (or a release build's
  start-up probe) has said Automation is granted, opening capture first reads the
  same permission report onboarding reads. A report saying Automation is not granted
  switches the panel to onboarding and skips the capture. Granted confirms the
  session; no report (a failed check) lets the capture run.
- **Settings › Permissions.** A status Label `Allowed`, `Not allowed`, `Not checked
  yet` (the check could not decide) or, while the check runs, `Checking…`, re-read
  off the main thread each time Settings is shown, and a Button containing `Open
  permission setup` that switches the panel to onboarding.

## Phase 1 view-state contract

What the Phase 1 views must expose for these scenarios to pass. It is derived from
the proofs and kept minimal: most proofs read the rendered accessibility tree, so
most of the contract is visible text. Where Phase 1 can choose, the Tauri panel's
behaviour (spec B1, `dist/panel.js`) is the default.

### Shared state: views get state via `app_state::get()`

Views get state via `app_state::get()` (`desktop-gpui/src/app_state.rs`, final
review I5): the one `ubilink_app::AppState` the shell builds at start-up from
`paths::data_dir()` — the graph in `<data dir>/ubilink-graph.sqlite` (the file
`seed_graph` writes), the adapter registry with `adapters.json`, the persisted
`plugins-state.json` disabled set. Pass it to ubilink-app's functions
(`list_links(app_state::get())`, `set_plugin_enabled(..)`, `export_graph(..)`, ...).
A view MUST NOT build its own `AppState`, open its own `LinkGraph` or copy
`desktop/src/main.rs`'s bootstrap: a settings toggle or a marked link must reach every
view, and that bootstrap's `probe_automation_permission(&OsaScriptRunner)` would raise
TCC prompts mid-probe. Automation builds start with the permission `NotDetermined`
and never run that probe; the onboarding view reads the `permission` fixture
(`crate::fixtures::path(Fixture::Permission)`) and the capture view the `capture`
fixture instead. `app_state::startup_error()` is the message to show when the on-disk
graph could not be opened (the state then runs on an in-memory graph). Unit tests
build a state with a temp dir (`app_state.rs`'s tests), never through `get()`.

### `state.view` fields (the route's `ViewState`, `state` verb)

`state.view` is the current route's own serialisable state (`views::state_for`). The
existing `view` and `phase` keys stay. Phase 1 adds exactly these:

| Route | Field | Type | Meaning | Used by |
|---|---|---|---|---|
| capture | `phase` | string | `"capturing"` from the moment the view is shown until the core answers (it is already `capturing` in the `trigger_hotkey` reply), then `"captured"` or `"failed"` | capture-view |
| browser | `filter_samples` | integer | how many filter re-computations (one per keystroke that changes the filter text) were timed since the browser view was last shown; 0 right after it opens | link-browser-filter |
| browser | `filter_p99_ms` | number or null | nearest-rank p99 (as `metrics.summary`) of those re-filter times, in ms, from the keystroke's dispatch to the filtered row model being ready | link-browser-filter |
| settings | `layout` | object | measured at prepaint (`views/layout.rs`), window points: `viewport` `{w, h}`; `{x, y, w, h, bottom}` for `adapters_box`, `last_adapter`, `export_button`, `import_button`, `reset_button`, `permissions_button` and, while the reset question shows, `reset_confirm_button` and `reset_cancel_button`; `{font_px}` for `login_label` and `login_status` (the text size each text resolves to). A key whose element is not drawn this frame is absent | settings-fit, settings-buttons, settings-login-text |
| browser | `layout` | object | measured at paint (`views/layout.rs` `Layout::text_field`), window points: `filter` holds `field` (the filter field's bounds), `content` (those bounds less the field's border and vertical padding: the box its text is clipped to) and `text` (the line box the field laid its text out in this frame), each `{x, y, w, h, bottom}`, and `centre_offset` (&#124;text centre − field centre&#124;, points) | browser-filter-text |
| capture | `layout` | object | as settings: `viewport`, and with backlinks `backlinks_box` and `last_backlink` | capture-fit |
| every view | `theme_samples` | object | `{surface, text, primary, accent}`, each `{x, y, w, h}` in window points from the panel's top-left, or `null` while not drawn: a region of the opaque card the view paints in `color.windowBackground`, a region around a run of primary text (`color.label`) drawn on that card, a region inside the primary (default) button's fill clear of its label and corners, and (Settings only) a region around a checked, enabled Switch, whose track is the system accent; each at least 24 × 24 pt (the probe fails a smaller one) | theming, accent |

The browser's first-frame time is not a view-state field: the browser records a
`view_first_frame` sample (route `"browser"`, timed at present) in the metrics ring
(`automation::store::record_metric`), read through `metrics.summary.view_first_frame`.

### Accessibility text and behaviour

All asserted text must reach the tree as `text!` (A11Y-NOTES.md). "Label" below means
a `text!` element (role `Label`).

- **Capture (R1-R3).** A Label containing `Capturing` while capturing; Labels with the
  captured resource's name and its URI; on failure a Label whose text is the capture
  error's message verbatim (the fixture's sentence says what failed and what to do).
  Enter or Cmd-C in the captured state copies the URI to the pasteboard and shows a
  Label containing `Copied`, kept visible at least 1 s if the panel then dismisses
  itself. Backlinks are the only `ListItem` rows in the capture view, newest link
  first, each row's subtree holding the resource's name and URI as Labels.
- **Browser (R4-R8).** Rows are gpui-component `List` rows (`ListItem`, with List's own
  `selected` and `size_of_set`), each row's subtree holding the name and the URI as
  Labels; no other `ListItem`s in the view. Top-level order is ubilink-app's
  `list_links` (`updated_at` DESC, then rowid DESC), the Tauri browser's. Selection
  starts on row 1. A `TextInput` filter is focused when the browser opens and stays focused; printable
  keys type into it, Backspace deletes, the filter ranks by `probe.py rank()`
  (fixtures/README.md) and drops non-matches. Down/Up move the selection, Enter opens
  the selected URI through the core opener, End shows the last row. Right drills into
  the selected link's connections (`links_of`, by name); Left returns with the
  previous selection. A Label reads `Your links` at the top level and
  `Linked to <names joined by " › ">` when drilled. ⌘L (key `cmd-l`; spec R8, ruling
  R-n) marks the selected link and, on a second link, links the two (plain L types
  into the filter, as in Tauri); a Label then reads `Linked “<A name>” ↔ “<B name>”`.
  Because the filter keeps focus, the focused Input swallows Up/Down/Home/End (and
  sees Enter/Escape/Right/Left first): the browser intercepts them on a parent with
  `capture_action::<MoveUp/MoveDown/MoveHome/MoveEnd/Enter/Escape>` plus
  `cx.stop_propagation()`, handles Right/Left with bubble-phase
  `on_action::<MoveRight/MoveLeft>` (they reach it at the filter's text edges), and
  ⌘L with a parent `on_key_down`. The per-key table and the proven reference are in
  A11Y-NOTES.md ("Navigating a list while an Input keeps focus", a11y-spike.json).
- **Onboarding (R9).** One `ListItem` per permission capture needs: since rc.4
  (review S1) only `Automation`; no Accessibility step, since nothing ubilink does
  needs it, and "All set" depends on Automation alone. Each subtree names the
  permission and a status Label `Not granted` or exactly `Granted`; rows stay listed
  when granted. Each row has a Button whose
  text contains the permission's name and opens `settings_pane_url(<permission>)`
  through the opener; Tab reaches it and Enter activates it. `R` re-reads the
  permission state in the same window.
- **Settings (R10-R13).** One `Switch` per adapter whose accessible name is exactly
  the app name (`Arc`, `Bear`, ...; built-ins Finder and Safari locked on), reachable
  with Tab and toggled with Space, persisted to `plugins-state.json` in ubilink-app's
  format. A stats Label exactly `<resources> link(s) · <links> connection(s)` (Tauri
  wording, e.g. `5 links · 3 connections`, `0 links · 0 connections`), re-read each
  time settings is shown. Buttons whose text contains `Export`, `Import` and `Reset`,
  reachable with Tab and activated with Enter; export writes to and import reads from
  the `export_path`/`import_path` fixture paths when set. Reset first shows a Label
  `Delete all captured links and connections?` with Buttons `Delete everything` and
  `Cancel` (no Dialog role is asserted: A11Y-NOTES.md). A `Switch` named
  `Launch at login` plus a status Label whose text is exactly one of `Enabled`,
  `Requires approval`, `Not registered`, `Not found` (`login_item::Status::label`);
  with requires-approval or not-found, a Label with the report's `explanation`
  verbatim. The switch is on for Enabled and Requires approval, off otherwise. See the
  launch-at-login seam below. A Label exactly `Ubilink <version>`, the crate version
  (Phase 2b, the Tauri panel's settings footer).

### Launch-at-login seam (R13, spec D6, ruling R-o)

Probes never register a real login item (O4), so the settings view must route every
SMAppService call through `desktop-gpui/src/login_item.rs` (Phase 0 code, all builds):

| Call | Returns | The view must |
|---|---|---|
| `login_item::status()` | `Option<Result<Report, String>>` | call it each time settings is shown; `Some(Ok(r))` → show `r.status.label()` (and `r.explanation`); `Some(Err(e))` → show `e` (U3); `None` → read `SMAppService.mainApp.status` itself |
| `login_item::register()` | same | call it when the switch is turned on, show the result; `None` → `SMAppService.mainApp.register()` |
| `login_item::unregister()` | same | call it when the switch is turned off, show the result; `None` → `SMAppService.mainApp.unregister()` |

An automation build always gets `Some(..)`: the fixture's result with a `login_item`
fixture loaded, else `Some(Err("no login_item fixture loaded"))` (final review I9: it
never falls through to the real SMAppService, O4). A release build always gets `None`. The seam treats the fixture file
(`login-item-*.json`, fixtures/README.md) as the instance's login-item store: register
sets `status` to the file's `on_register`, unregister to `not_registered`, and both
append their name to `calls`, which the scenario reads back with `file` steps. The
view must not cache the status across shows or call SMAppService when the seam
answers.
- **Every view (R14, D26).** Colours follow `set_appearance` live, without a new window or
  process. Expected samples (ΔE ≤ 3), with the accent pinned to `#007aff` by
  `accent-blue.json`: light card `#ffffff`, text `#262626` (label over the card),
  primary button `#0072ef`, checked switch `#007aff`; dark card `#1e1e1e`, text
  `#dddddd`, primary button `#0072ef`, checked switch `#007aff`.
- **Accent (D26).** `appearance.accent` (in `state`) is the accent the theme was built
  from, `#rrggbb`. Automation builds take it from the `accent` fixture when one is
  loaded (loading one re-themes open windows in place, the path the system's
  colour-change notification takes); otherwise, as release builds always do, from
  `NSColor.controlAccentColor`. `appearance.primary` and `appearance.button_primary`
  are the live theme's colours (`#rrggbb`); `appearance.accent_refreshes` counts
  accent re-themes. The automation-only verb `post_colors_changed` posts
  `NSSystemColorsDidChangeNotification` in-process, so the observer → shell inbox →
  re-theme path runs (`accent.json`).
- **Material (D26).** `shell.material` is what sits behind the panel's content, read
  back from the live window: `"glass"` (NSGlassEffectView, macOS 26+), `"vibrancy"`
  (NSVisualEffectView, older systems) or `"unknown"` before a panel is read.
  `shell.json`, `panel-hotkey.json` and `performance.json` assert `"glass"`, so they
  need macOS 26 or later; on an older Mac those steps fail by design.

## PERFORMANCE.md format

`python3 scripts/probe.py budget [--write docs/PERFORMANCE.md]` is the measurement:
the release app exactly as `performance.json`'s `release_budget` step measures it,
then panel-hotkey's 50 triggers on a fresh automation instance. It prints the table
below and, with `--write`, puts it in the file between `<!-- probe.py budget: begin -->`
and `<!-- probe.py budget: end -->` (replacing an earlier block, keeping everything
else). It writes nothing unless every number was measured and within budget. Phase
1's `make budget` must run it: `performance.json` requires `make -n budget` to print
a line containing `scripts/probe.py budget`, so a no-op target fails.

`docs/PERFORMANCE.md` (Phase 1 R15, written by `make budget`'s run) must contain one
Markdown table row per key below, in any order, anywhere in the file; other lines and
rows are ignored. Cells: the key, the budget exactly as shown, and the measured value
as a plain number in the key's unit (no unit text).

```
| Metric | Budget | Measured |
|---|---|---|
| idle_cpu_percent | < 0.5 | 0.08 |
| physical_footprint_mb | < 80 | 42.1 |
| stripped_binary_mb | < 20 | 12.3 |
| hotkey_p50_ms | < 80 | 28 |
| hotkey_p99_ms | < 200 | 61 |
```

| Key | Measured how (by `performance.json`) | Agrees when within |
|---|---|---|
| `idle_cpu_percent` | release app, CPU-time delta / 60 s wall, % of one core | ± 0.25 points |
| `physical_footprint_mb` | release app, `vmmap --summary` Physical footprint, MiB | ± max(10 MB, 15 %) |
| `stripped_binary_mb` | release binary after `strip`, MiB | ± 1 MB |
| `hotkey_p50_ms` | automation app, 50 triggers, `metrics.summary.hotkey_to_first_frame.p50` | ± max(20 ms, 50 %) |
| `hotkey_p99_ms` | same, `.p99` | ± max(40 ms, 50 %) |

Every measured value in the doc must itself be under its budget, and `make -n budget`
must succeed and show a recipe line running `scripts/probe.py budget`. The tolerances are wide for latency on purpose: the doc records a
typical run, the probe re-measures on whatever machine runs it.
