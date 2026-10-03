# Probe scenarios

Each `<name>.json` here is a scenario that `scripts/probe.py run <name>` runs against
fresh, isolated automation instances of `ubilink-next` (GPUI Phase 0 R8). The Phase 1
spec's `[proof:]` clauses become scenarios in this format, so the format is part of
the contract. Change it only together with `scripts/probe.py`, and keep
`scripts/test_probe.py` passing.

```
python3 scripts/probe.py run shell                  # one instance
python3 scripts/probe.py run shell --instances 2    # two at once, plus isolation checks
python3 scripts/probe.py run shell panel-hotkey     # several scenarios in one run
python3 scripts/probe.py list --expect 15           # the Phase 1 scenarios, each run, with results
python3 -m unittest scripts/test_probe.py           # the engine's self-tests (no app)
```

## What a run does

1. Builds the automation binary once with
   `cargo build --locked --message-format=json-render-diagnostics --features automation --manifest-path desktop-gpui/Cargo.toml`
   using `CARGO_TARGET_DIR=~/Library/Caches/ubilink-probe/target`. The first time
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
   `desktop-gpui/`, `app/`, `src-tauri/` or `design/` have uncommitted changes) at
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
   at once. A second probe waits, up to 15 minutes, and prints who holds the lock.
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

   Any `UBILINK_*` variable exported in your shell is dropped first.
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
  `permission`, `graph_dir`, `export_path`, `import_path` and `login_item`
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
| `pixel` | `{x, y, w?, h?}` in window points from the top-left, or a dot path into `state` where the view reports such a region | `color` (`#rrggbb`, required), `delta_e` (default 3.0), `match` (`mean`, default, or `best`) | The region's mean sRGB colour (`best`: its closest single pixel, for a text run whose antialiased edges are not the text colour) must be within CIE76 ΔE of `color`. A state path that holds no region is a FAIL (the view did not report it). Uses the same capture as `ocr`, so it also needs Screen Recording. |
| `pasteboard` | `"set_nonce"` or `"read"` | `expect`, `changed` | `set_nonce` puts a unique string on the pasteboard and records the change count. `read` compares `pbpaste` against `expect` (a literal, which may be `"{nonce}"`, or a matcher). `changed: true/false` requires the change count to have risen (or not) since the last pasteboard step. |
| `graph` | `true` | `db`, `export`, `links`, `no_links`, `link_count`, `resource_count`, `timeout_ms` | Reads the instance's graph read-only. The default is `{data_dir}/ubilink-graph.sqlite` (ubilink-core's file). `db` names another file, and `export` reads a Settings > Export JSON instead. `links`/`no_links` are `[[uriA, uriB], ...]` unordered pairs. |
| `file` | path | `exists` (default true), `expect`, `timeout_ms` | The file exists (or not). With `expect`, it parses as JSON and meets the path expectations. |
| `winframe` | `"any"`, `"cursor"`, `"state"`, `"gone"` or a display index | `timeout_ms` (default 2000) | Finds the panel's frame through Quartz `CGWindowListCopyWindowInfo`, matched by `window_id`. It must lie within any display, the display under the cursor, the display `state.shell.display_id` names, or display *n*. `"gone"` instead requires the window found by the most recent `winframe` (or `window_id` call) for the current panel to be absent from Quartz's on-screen list (a hide checked at the window server, not just in `state`). A call showing a new panel (hidden → visible) forgets the old number, and a passing `gone` uses it up, so with no such window it fails. |
| `warp_cursor` | display index, or `{display, x, y}` (fractions, default 0.5) | | Moves the cursor with `CGWarpMouseCursorPosition`. No event is posted and no Accessibility grant is needed. |
| `activate_other` | `true` | `settle_ms` (default 300) | A real loss of key status for the panel: launches the probe's own activator (`scripts/probe-helpers/activator.swift`, compiled like the other helpers), a minimal app that makes itself active with a 1-point key window, waits for it to report `active`, holds for `settle_ms`, then quits it. No user app is ever launched (O4), and none is left running. A real deactivation test necessarily takes focus from your app for that moment. If the activator cannot become active in 8 s the step is ENV-BLOCKED. |
| `restart` | `true` | | Stops the instance and relaunches it with the same temp root (data survives), then replays every fixture loaded so far, since fixtures live in app memory. |
| `sleep_ms` | milliseconds | | Waits. Prefer `retry_ms` or `timeout_ms`. |
| `key_until` | a node query (as in `see`) | `key` (default `tab`), `max_presses` (default 40, at most 200), `route` | Checks the query, then presses `key` (the `key` verb) and checks again, until it holds; FAIL after `max_presses`. "This control is reachable from the keyboard" (Phase 1 U4) without the probe hard-coding a view's tab order: the target is usually `{"role": "Button", "text": "Export", "state": {"focused": true}}`. |
| `seed_graph` | a `ubilink.graph.v3` fixture path | `replace` (default false) | Writes the fixture into the instance's `{data_dir}/ubilink-graph.sqlite` with ubilink-core's own schema and the same statements `LinkGraph::import_json` runs (upsert by URI, guids kept, `updated_at = created_at`, resources in file order, one row per unordered link, dangling and self links skipped); `replace` first deletes every link and resource like `LinkGraph::clear`. Merges into a live instance's graph (SQLite locking makes it one more writer). It skips only the core's URI validation, so fixtures use allowed schemes. `test_probe.py` pins the schema to `src-tauri/src/graph.rs`. |
| `process` | `"unchanged"` | | The instance is still the process this scenario launched (or last restarted), and alive: "the same pid" (Phase 1 R14). |
| `screenshot` | a file-name-safe label | | Captures the panel like `ocr` and keeps the PNG at `~/Library/Caches/ubilink-probe/shots/<run>/<scenario>-<label>.png` (instance temp roots are deleted; the run prints the directory). Needs Screen Recording. |
| `release_budget` | `true` | `cpu_pct_lt`, `footprint_mb_lt`, `binary_mb_lt` (all required), `idle_s` (default 60), `settle_s` (default 5) | Builds the stripped release build (no automation channel) and launches it as a second isolated instance (see [Release-build instances](#release-build-instances)), waits `settle_s`, then measures idle CPU as the `ps -o time=` CPU-time delta over `idle_s` seconds of wall time, physical footprint with `vmmap --summary`'s `Physical footprint:` line (Activity Monitor's Memory number) at the end, and the binary's size after `strip` (a copy). Each must be under its budget. Saves `idle_cpu_percent`, `physical_footprint_mb`, `stripped_binary_mb`. A `vmmap` refusal is ENV-BLOCKED. |
| `perf_doc` | path of `docs/PERFORMANCE.md` (use `{repo}`) | `compare` (default false), `make_target` | Parses the doc's budget table ([PERFORMANCE.md format](#performancemd-format)): every row present once, each budget cell exactly as specified, each measured value under its budget. `compare` also requires each value to agree with this run's saved measurement within the row's tolerance. `make_target` requires `make -n <target>` to exit 0 at the repo root (a dry run: make prints the recipe but runs only `$(shell ...)` and `+` lines). |

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

### Text matching (`see`, `ocr`, the `text` operator)

Both sides are NFKC-normalised (this turns `…` into `...`), casefolded and
whitespace-collapsed. The expected text must then be a substring of the node's text,
or equal to it with `exact`. A narrow panel truncates by design, so a node ending in
an ellipsis also matches when the visible text before the ellipsis ends with at
least 6 characters (or all) of the start of the expected text. For example,
`"Capture: https://exa…"` matches `https://example.com/page`. A leading ellipsis
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
instance's steps. ENV-BLOCKED steps are skipped and the run continues.

| Exit | Meaning |
|---|---|
| 0 | every step passed |
| 1 | a product FAIL, or the harness could not build or launch |
| 2 | a scenario error (bad file, unknown step or option) |
| 3 | nothing failed, but some step was ENV-BLOCKED (missing Screen Recording, too few displays, display asleep, focus taken twice, no swiftc) |

With `--instances N` (N ≥ 2), every instance runs all the steps. While all of them
are still running, the probe then checks that each one answers `state` with its own
`data_dir`, that each `opened` sink answers, and that pids, sockets, data
dirs and fixture dirs are all distinct (R8, O1).

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
scenario files, never read from `design/`, `src-tauri/` or `app/` at run time.

## Phase 1 view-state contract

What the Phase 1 views must expose for these scenarios to pass. It is derived from
the proofs and kept minimal: most proofs read the rendered accessibility tree, so
most of the contract is visible text. Where Phase 1 can choose, the Tauri panel's
behaviour (spec B1, `dist/panel.js`) is the default.

### `state.view` fields (the route's `ViewState`, `state` verb)

`state.view` is the current route's own serialisable state (`views::state_for`). The
existing `view` and `phase` keys stay. Phase 1 adds exactly these:

| Route | Field | Type | Meaning | Used by |
|---|---|---|---|---|
| capture | `phase` | string | `"capturing"` from the moment the view is shown until the core answers (it is already `capturing` in the `trigger_hotkey` reply), then `"captured"` or `"failed"` | capture-view |
| browser | `filter_samples` | integer | how many filter re-computations (one per keystroke that changes the filter text) were timed since the browser view was last shown; 0 right after it opens | link-browser-filter |
| browser | `filter_p99_ms` | number or null | nearest-rank p99 (as `metrics.summary`) of those re-filter times, in ms, from the keystroke's dispatch to the filtered row model being ready | link-browser-filter |
| every view | `theme_samples` | object | `{surface, text, accent}`, each `{x, y, w, h}` in window points from the panel's top-left: a region of an opaque surface the view paints with `color.uiBg`'s RGB at full opacity, a region around a run of primary text (`color.text1`) drawn on that surface, and a region inside a control filled with `color.accent` | theming |

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
  starts on row 1. A `TextInput` filter is focused when the browser opens; printable
  keys type into it, Backspace deletes, the filter ranks by `probe.py rank()`
  (fixtures/README.md) and drops non-matches. Down/Up move the selection, Enter opens
  the selected URI through the core opener, End shows the last row. Right drills into
  the selected link's connections (`links_of`, by name); Left returns with the
  previous selection. A Label reads `Your links` at the top level and
  `Linked to <names joined by " › ">` when drilled. ⌘L (key `cmd-l`; spec R8, ruling
  R-n) marks the selected link and, on a second link, links the two (plain L types
  into the filter, as in Tauri); a Label then reads `Linked “<A name>” ↔ “<B name>”`.
- **Onboarding (R9).** One `ListItem` per missing permission (`Accessibility`,
  `Automation`), each subtree naming the permission and a status Label `Not granted`
  or exactly `Granted`; rows stay listed when granted. Each row has a Button whose
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
  launch-at-login seam below.

### Launch-at-login seam (R13, spec D6, ruling R-o)

Probes never register a real login item (O4), so the settings view must route every
SMAppService call through `desktop-gpui/src/login_item.rs` (Phase 0 code, all builds):

| Call | Returns | The view must |
|---|---|---|
| `login_item::status()` | `Option<Result<Report, String>>` | call it each time settings is shown; `Some(Ok(r))` → show `r.status.label()` (and `r.explanation`); `Some(Err(e))` → show `e` (U3); `None` → read `SMAppService.mainApp.status` itself |
| `login_item::register()` | same | call it when the switch is turned on, show the result; `None` → `SMAppService.mainApp.register()` |
| `login_item::unregister()` | same | call it when the switch is turned off, show the result; `None` → `SMAppService.mainApp.unregister()` |

`Some(..)` happens only in an automation build with a `login_item` fixture loaded; a
release build always gets `None`. The seam treats the fixture file
(`login-item-*.json`, fixtures/README.md) as the instance's login-item store: register
sets `status` to the file's `on_register`, unregister to `not_registered`, and both
append their name to `calls`, which the scenario reads back with `file` steps. The
view must not cache the status across shows or call SMAppService when the seam
answers.
- **Every view (R14).** Colours follow `set_appearance` live, without a new window or
  process. Expected samples (ΔE ≤ 3): light surface `#fafafc`, text `#1e1e1e`
  (text1 over that surface), accent `#007aff`; dark surface `#1c1c1e`, text `#f6f6f6`,
  accent `#0a84ff`.

## PERFORMANCE.md format

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
must succeed. The tolerances are wide for latency on purpose: the doc records a
typical run, the probe re-measures on whatever machine runs it.
