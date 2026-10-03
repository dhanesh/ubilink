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
   `cargo build --locked --features automation --manifest-path desktop-gpui/Cargo.toml`
   using `CARGO_TARGET_DIR=~/Library/Caches/ubilink-probe/target`. The first time
   that directory is empty, it is seeded from `desktop-gpui/target` with `cp -c -R`,
   an APFS clone that costs seconds and no disk space. Later builds, including ones
   from fresh git clones, only rebuild the path crates.
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
   - `UBILINK_DATA_DIR=<root>/data`, which the app pre-loads as the `graph_dir`
     fixture (state shows it under `fixtures.graph_dir`)
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
which is shown in the report.

| Step | Value | Options | Checks |
|---|---|---|---|
| `call` | verb name | `args`, `expect`, `expect_error`, `retry_ms` | Sends `{"verb", "args"}` and requires `ok`, then every `expect` entry against `result`. `expect_error` (substring, or `true`) requires `ok: false` instead. `retry_ms` re-sends until the step passes or the time runs out. It is allowed only for read-only verbs: `state`, `metrics`, `window_id`, `a11y`, `opened`. |
| `see` | `{role?, text?}` or a list of them | `absent`, `exact`, `count`, `min_count`, `max_count`, `timeout_ms` (default 3000) | Polls `a11y` (GPUI's AccessKit tree, read in-process with no TCC grant, spec TN5) until each query matches a node, or matches none with `absent`. Roles compare case-insensitively, ignoring punctuation. Text matching is described below. |
| `ocr` | text or list | `absent`, `exact` | Runs `screencapture -l<window_id>` on the panel, then Vision OCR (`scripts/ocr.swift`), and requires each text in some line (or in all lines joined). Needs Screen Recording. |
| `pixel` | `{x, y, w?, h?}` in window points from the top-left | `color` (`#rrggbb`, required), `delta_e` (default 3.0) | The region's mean sRGB colour must be within CIE76 ΔE of `color`. Uses the same capture as `ocr`, so it also needs Screen Recording. |
| `pasteboard` | `"set_nonce"` or `"read"` | `expect`, `changed` | `set_nonce` puts a unique string on the pasteboard and records the change count. `read` compares `pbpaste` against `expect` (a literal, which may be `"{nonce}"`, or a matcher). `changed: true/false` requires the change count to have risen (or not) since the last pasteboard step. |
| `graph` | `true` | `db`, `export`, `links`, `no_links`, `link_count`, `resource_count`, `timeout_ms` | Reads the instance's graph read-only. The default is `{data_dir}/ubilink-graph.sqlite` (ubilink-core's file). `db` names another file, and `export` reads a Settings > Export JSON instead. `links`/`no_links` are `[[uriA, uriB], ...]` unordered pairs. |
| `file` | path | `exists` (default true), `expect`, `timeout_ms` | The file exists (or not). With `expect`, it parses as JSON and meets the path expectations. |
| `winframe` | `"any"`, `"cursor"`, `"state"` or a display index | `timeout_ms` (default 2000) | Finds the panel's frame through Quartz `CGWindowListCopyWindowInfo`, matched by `window_id`. It must lie within any display, the display under the cursor, the display `state.shell.display_id` names, or display *n*. |
| `warp_cursor` | display index, or `{display, x, y}` (fractions, default 0.5) | | Moves the cursor with `CGWarpMouseCursorPosition`. No event is posted and no Accessibility grant is needed. |
| `activate_other` | app name, or `true` for Finder | `settle_ms` (default 300) | Runs `open -a <app>`, a real loss of key status for the panel. |
| `restart` | `true` | | Stops the instance and relaunches it with the same temp root (data survives), then replays every fixture loaded so far, since fixtures live in app memory. |
| `sleep_ms` | milliseconds | | Waits. Prefer `retry_ms` or `timeout_ms`. |

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

## Report and exit codes

The report has one line per step: `PASS`, `FAIL`, `ENV-BLOCKED` or `ERROR`. A
failure also prints the expected and actual values. The first FAIL stops that
instance's steps. ENV-BLOCKED steps are skipped and the run continues.

| Exit | Meaning |
|---|---|
| 0 | every step passed |
| 1 | a product FAIL, or the harness could not build or launch |
| 2 | a scenario error (bad file, unknown step or option) |
| 3 | nothing failed, but some step was ENV-BLOCKED (missing Screen Recording, too few displays, no swiftc) |

With `--instances N` (N ≥ 2), every instance runs all the steps. While all of them
are still running, the probe then checks that each one answers `state` with its own
`fixtures.graph_dir`, that each `opened` sink answers, and that pids, sockets, data
dirs and fixture dirs are all distinct (R8, O1).

## Limits

- `pasteboard: set_nonce` saves your pasteboard **text** first and restores it at the
  end of the run. Non-text contents, such as images or files, are not restored.
- `ocr` and `pixel` need Screen Recording for the process running `probe.py`.
  Without it they are ENV-BLOCKED. Text proofs should use `see`.
- `warp_cursor` and `activate_other` act on the real desktop. That is why every run
  holds the desktop lock.
