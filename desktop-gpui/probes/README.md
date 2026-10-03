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
    "files":    {"{data_dir}/ubilink-graph.sqlite": "graphs/three-backlinks.sqlite"},
    "fixtures": {"capture": "capture/delayed-300ms.json"}
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
- `setup.fixtures` sends `load_fixture {kind, path}` after launch. Relative paths
  resolve against the instance's fixture copy. The kinds are `capture`,
  `permission`, `graph_dir`, `export_path` and `import_path`
  (`desktop-gpui/src/automation/protocol.rs`).
- Unknown top-level keys, unknown step types and unknown step options are
  **scenario errors** (exit 2). A typo can never make a step pass.

### Placeholders

Strings in `args`, `expect`, paths and `setup` may use `{root}`, `{data_dir}`,
`{fixtures}`, `{probes}` (this directory), `{instance}` (the `UBILINK_INSTANCE`
value) and, after a `pasteboard: set_nonce`, `{nonce}`.

### Steps

Each step is an object with exactly one step key. Any step may also have a `name`,
which is shown in the report, and a `block` id: consecutive steps with the same
`block` are one iteration, the unit a focus-theft retry re-runs (see
[Unattended guards](#unattended-guards)).

| Step | Value | Options | Checks |
|---|---|---|---|
| `call` | verb name | `args`, `expect`, `expect_error`, `retry_ms` | Sends `{"verb", "args"}` and requires `ok`, then every `expect` entry against `result`. `expect_error` (substring, or `true`) requires `ok: false` instead. `retry_ms` re-sends until the step passes or the time runs out. It is allowed only for read-only verbs: `state`, `metrics`, `window_id`, `a11y`, `opened`. |
| `see` | a query, or a non-empty list of them (see [`see` queries](#see-queries)) | `absent`, `exact`, `count`, `min_count`, `max_count`, `route`, `timeout_ms` (default 3000) | Polls `a11y` (GPUI's AccessKit tree, read in-process with no TCC grant, spec TN5) until every query holds. `route` first requires `state.route` to equal it. `absent` also requires the tree to have at least one node other than the Window, so an empty tree can't pass it. Roles compare case-insensitively, ignoring punctuation. Text matching is described below. |
| `ocr` | text or list | `absent`, `exact` | Runs `screencapture -l<window_id>` on the panel, then Vision OCR (`scripts/ocr.swift`), and requires each text in some line (or in all lines joined). Needs Screen Recording. |
| `pixel` | `{x, y, w?, h?}` in window points from the top-left | `color` (`#rrggbb`, required), `delta_e` (default 3.0) | The region's mean sRGB colour must be within CIE76 ΔE of `color`. Uses the same capture as `ocr`, so it also needs Screen Recording. |
| `pasteboard` | `"set_nonce"` or `"read"` | `expect`, `changed` | `set_nonce` puts a unique string on the pasteboard and records the change count. `read` compares `pbpaste` against `expect` (a literal, which may be `"{nonce}"`, or a matcher). `changed: true/false` requires the change count to have risen (or not) since the last pasteboard step. |
| `graph` | `true` | `db`, `export`, `links`, `no_links`, `link_count`, `resource_count`, `timeout_ms` | Reads the instance's graph read-only. The default is `{data_dir}/ubilink-graph.sqlite` (ubilink-core's file). `db` names another file, and `export` reads a Settings > Export JSON instead. `links`/`no_links` are `[[uriA, uriB], ...]` unordered pairs. |
| `file` | path | `exists` (default true), `expect`, `timeout_ms` | The file exists (or not). With `expect`, it parses as JSON and meets the path expectations. |
| `winframe` | `"any"`, `"cursor"`, `"state"`, `"gone"` or a display index | `timeout_ms` (default 2000) | Finds the panel's frame through Quartz `CGWindowListCopyWindowInfo`, matched by `window_id`. It must lie within any display, the display under the cursor, the display `state.shell.display_id` names, or display *n*. `"gone"` instead requires the window found by the most recent `winframe` (or `window_id` call) for the current panel to be absent from Quartz's on-screen list (a hide checked at the window server, not just in `state`). A call showing a new panel (hidden → visible) forgets the old number, and a passing `gone` uses it up, so with no such window it fails. |
| `warp_cursor` | display index, or `{display, x, y}` (fractions, default 0.5) | | Moves the cursor with `CGWarpMouseCursorPosition`. No event is posted and no Accessibility grant is needed. |
| `activate_other` | `true` | `settle_ms` (default 300) | A real loss of key status for the panel: launches the probe's own activator (`scripts/probe-helpers/activator.swift`, compiled like the other helpers), a minimal app that makes itself active with a 1-point key window, waits for it to report `active`, holds for `settle_ms`, then quits it. No user app is ever launched (O4), and none is left running. A real deactivation test necessarily takes focus from your app for that moment. If the activator cannot become active in 8 s the step is ENV-BLOCKED. |
| `restart` | `true` | | Stops the instance and relaunches it with the same temp root (data survives), then replays every fixture loaded so far, since fixtures live in app memory. |
| `sleep_ms` | milliseconds | | Waits. Prefer `retry_ms` or `timeout_ms`. |

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

Three query shapes:

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
single-instance socket exists, and `call()` refuses. No scenario step uses it yet.

## Limits

- `pasteboard: set_nonce` saves your pasteboard **text** first and restores it at the
  end of the run. Non-text contents, such as images or files, are not restored.
- `ocr` and `pixel` need Screen Recording for the process running `probe.py`.
  Without it they are ENV-BLOCKED. Text proofs should use `see`.
- `warp_cursor` and `activate_other` act on the real desktop. That is why every run
  holds the desktop lock. `activate_other` briefly takes focus from whatever you are
  using.
