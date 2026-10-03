---
name: verify-ubilink
description: >-
  Prove a change to ubilink-next (the GPUI macOS menu-bar app in desktop-gpui/: tray,
  pop-up panel, capture / browser / settings / onboarding views) works by launching
  isolated automation instances, driving them through the automation socket the way the
  hotkeys and tray do, and recording SHA-bound evidence (state JSON, the accessibility
  tree, panel screenshots). Use when asked to verify, prove or demo a desktop-gpui
  feature, when factory-conductor's evidence gate needs a verifier record for a touched
  feature, or when a feature map entry under features/ must be added or re-proven.
  Tests passing is not verification; this is.
---

# verify-ubilink

ubilink-next is the GPUI rewrite of the ubilink desktop app: a menu-bar (accessory) app
with no Dock icon whose global hotkeys and tray open one pop-up, blurred panel that
routes between the `capture`, `browser`, `settings` and `onboarding` views. A user
touches it through hotkeys, the tray and the keyboard inside the panel. An agent cannot
press a real global hotkey (instances run with `UBILINK_SKIP_HOTKEYS=1` so they never
grab the user's keys), so it drives the **automation channel** instead: an
`--features automation` build serves a per-instance Unix socket that accepts exactly
eleven verbs (`open_view`, `key`, `trigger_hotkey`, `resign_key`, `set_appearance`,
`load_fixture`, `state`, `metrics`, `window_id`, `a11y`, `opened`) and runs each through
the same code path the real input takes (`desktop-gpui/src/automation/protocol.rs`).

The harness is `scripts/ubilink_instance.py` in this skill. It is a thin split of
`scripts/probe.py`'s engine that keeps an instance alive between commands, so you can
launch, doctor, drive and record as separate steps. It uses the same warm locked build,
the same `Instance` isolation and the same desktop lock as probe.py, and it runs every
drive step through probe.py's own step runners, matchers and focus-theft guard. Every
harness command prints JSON on stdout; build and lock chatter goes to stderr. The
scenario runner `python3 scripts/probe.py run <scenario>` stays the regression tool;
this skill is how a verifier proves one head. Step and matcher syntax:
`desktop-gpui/probes/README.md`.

## Session variables

`SKILL` is the absolute path to this skill that you were given (under factory-conductor:
`<worktree>/desktop-gpui/.claude/skills/verify-ubilink`). The checkout that holds it is
the one you prove: the harness builds that checkout's code, and the recorder binds
evidence to that checkout's `HEAD`. **cd into that checkout before anything else**,
because the doctor fails when your shell's `HEAD` is not the skill's checkout's `HEAD`.

```sh
SKILL=/absolute/path/you/were/given/desktop-gpui/.claude/skills/verify-ubilink
WT="${SKILL%/desktop-gpui/.claude/skills/verify-ubilink}"
cd "$WT"
H="$SKILL/scripts/ubilink_instance.py"
VERIFIER=your-verifier-id
TASK=R0   # the task id your brief names (e.g. R4); "" when working alone
INSTANCE="$(python3 "$H" name --verifier "$VERIFIER" a)"
INSTANCE_B="$(python3 "$H" name --verifier "$VERIFIER" b)"
```

`TASK` picks the scenario set of the three features two Phase 1 tasks share
(link-browser R4+R5, browser-navigation R6+R7, graph-data R11+R12; final review C1):
at the first task's head only its own scenario exists in the product yet, so its
recipe runs that one; at every other head (and when `TASK` is empty) all of them.

`VERIFIER` is who records the proof. Under factory-conductor it is the verifier id the
verifier brief gives you, and it MUST NOT be the owner that wrote the change. Working
alone, use one stable name for the whole session (for example your session's agent id),
never the author of the change. `name` turns it into instance names unique to you
(`v<8 hex of the verifier id>-a`), because under factory-conductor several verifiers
share one run directory. Under factory-conductor also export
`VERIFY_EVIDENCE_DIR=<run root>/.verify` (the brief says which), so records land where
the conductor reads them; the harness keeps its run state in the sibling `.verify-run/`.

## Launch

```sh
python3 "$H" launch --instance "$INSTANCE" --verifier "$VERIFIER"
python3 "$H" launch --instance "$INSTANCE_B" --verifier "$VERIFIER"
python3 "$H" list
```

What `launch` does, in order:

0. Reaps orphans (probe.py's `reap_orphans`, final review I7): an instance under
   `$TMPDIR/ubprobe-*` whose owning `probe.py run` is dead (SIGKILLed by a timeout) is
   terminated, after checking its pid still runs the recorded binary. Harness
   instances are reaped only when no claim accounts for them.
1. Claims `.verify-run/$INSTANCE/instance.json` with an O_EXCL create, naming you as
   the verifier. A live claim exits 3 ("already running"); a stale one (dead pid) is
   replaced.
2. Takes or joins the **session lease** `~/Library/Caches/ubilink-probe/verify-lease.json`
   (O_EXCL; it names one run directory, one verifier and that verifier's instances).
   Only one verifier at a time drives instances on this desktop, so two verifiers' GUI
   steps can never interleave between calls. If another verifier holds it, `launch`
   waits up to 300 seconds (`--lease-timeout`) and then exits 3: GUI proofs serialise on
   one desktop, so a second verifier backs off well inside the conductor's 600 s per
   command (final review I8); parallel verifiers overlap only their builds. Your last
   `stop` releases it. How a waiting `launch` treats someone else's lease:
   - **Stale:** none of its instances is alive and none of its harness commands is
     running. The lease is replaced; there is nothing to stop.
   - **Idle:** no harness command of the holder has run for 240 seconds, and none is
     running now. This is how a crashed verifier's orphans get cleaned up. The waiting
     `launch` reclaims the lease:
     - it stops the holder's instances by their recorded pids, each only after checking
       that pid still runs our ubilink-next binary
     - it removes their claims
     - it writes `.verify-run/reclaims/<time>-<verifier>.json` under the holder's run
       directory
     - it logs `RECLAIMED ...` on stderr
     - it takes the lease
   - **Heartbeat:** your `launch`, `call` (after every step) and `doctor` refresh the
     lease's `last_activity`, and mark your process active while they run. So however
     long one command takes (a cold build, a desktop-lock wait, a long proof), it never
     counts as idle. Only a gap of more than 10 minutes between your commands does.
   - **Reclaimed:** if your lease was reclaimed, your next command on that instance
     exits 3 with "lease reclaimed after idle" and drives nothing. Launch again; with
     your new launch, the earlier evidence no longer applies.

   The decision rules are unit-tested: `python3 "$SKILL"/scripts/test_ubilink_instance.py`.
3. Builds the automation binary with probe.py's `build_binary` and clones the produced
   executable to `.verify-run/$INSTANCE/ubilink-next-automation`. The build is
   `cargo build --locked --features automation` in the warm shared target dir
   (`$CARGO_TARGET_DIR` when exported, else `~/Library/Caches/ubilink-probe/target`),
   held under its target lock. `launch` prints
   the binary's sha256 and source commit on stderr. A warm build takes a few seconds; a
   cold one takes several minutes.
4. Under probe.py's desktop lock, starts the instance with probe.py's `Instance`
   isolation. Each instance gets:
   - its own temp root `$TMPDIR/ubprobe-vf-$INSTANCE-*`
   - `UBILINK_DATA_DIR=<root>/data` (the app's one data-dir resolver, `src/paths.rs`,
     honours it)
   - its own single-instance socket `UBILINK_INSTANCE_SOCKET=<root>/si.sock`
   - its own copy of `probes/fixtures/`
   - `UBILINK_SKIP_HOTKEYS=1`, with every other `UBILINK_*` variable from your shell
     dropped

**No network port.** ubilink-next opens no TCP port and has no login, so there is no
port to claim with `verify_evidence.py port` and no auth session to keep apart. The
automation socket plays the port's role: `$TMPDIR/ubilink-<uid>/vf-$INSTANCE-<hash>/auto.sock`,
with `UBILINK_INSTANCE` set to `vf-$INSTANCE-<6 hex of the run dir>`, so two run
directories that use the same name still get different sockets. Three things keep owners
apart: the O_EXCL claim, the session lease, and the app itself, which refuses to bind a
socket another live process is serving (`automation/socket.rs`). The per-instance data
store is the temp root's `data/`. The "session" is that data dir plus the fixture copy,
nothing else.

**Ready** means the automation socket answered a `state` request. `launch` waits up to
60 seconds for it (`--timeout` to change). It then prints `{"INSTANCE": {...}}` with the
pid, socket, data dir, binary sha256 and commit, and exits 0. If the process exits, or
the socket never answers within the timeout, `launch` prints the app log's tail, removes
the instance and exits 2: stop waiting and read that log. The instance keeps running
after `launch` returns (it is in its own session) until you clean it up.

Before the first launch on a host, run `python3 scripts/probe.py doctor` (read-only). It
tells you whether this Mac can host GUI probes at all: Screen Recording for screenshots,
an awake display, swiftc for the Quartz helper, the pinned Rust toolchain.

## Doctor

One read-only check per instance. Record it before the first drive and again after any
failure:

```sh
python3 "$H" doctor --instance "$INSTANCE"
python3 "$SKILL"/scripts/verify_evidence.py doctor --instance "$INSTANCE" --verifier "$VERIFIER" $(python3 "$H" doctor --instance "$INSTANCE" --as-args)
```

The first command prints `{"doctor": {"ok": ..., "checks": {...}, "detail": {...}}}`. It
exits 0 when every check passes and 1 when one fails. `--as-args` prints the same
verdict for the recorder: `--worktree <the skill's checkout> --ok|--fail --check
name=pass|fail ...`. The recorder then writes
`.verify/$INSTANCE/doctor/<sha>/doctor.json` (`DOCTOR: ok <path>`). The checks:

| check | passes when |
|---|---|
| `process` | the recorded pid is alive and still runs this instance's binary copy (not a reused pid) |
| `build` | all of these hold: the binary's sha256 is the one built at launch; the commit it was built from equals the skill's checkout's `HEAD` and your cwd's `HEAD`; and `desktop-gpui/`, `app/`, `src-tauri/` and `design/` were clean at build time and are clean now. Evidence is bound to `HEAD`, so a dirty or moved checkout cannot be proven: commit, then relaunch. A cwd in another checkout fails here, so cd into `$WT` |
| `endpoint` | the instance's automation socket answers `state` and reports this instance's own `data_dir` (the port check, for a socket) |
| `isolation` | `state.shell.hotkeys_skipped` is true and the data dir is inside the instance's temp root, never `~/Library/Application Support` |
| `desktop` | the login session's screen is not locked. On a locked screen no panel becomes active and every a11y read fails for a reason that is not the product's; `call` refuses to drive one (exit 3). A sleeping display is fine: `call` holds `caffeinate -d -i -u` like `probe.py run`, which wakes it |

The JSON also reports `screen_recording` (needed only for screenshots) and the current
`route`. There is no `auth` check because the app has no login.

## Drive

There are two ways to drive:

- **`scenario` is the acceptance proof, and it is the preferred one.** It runs a frozen
  scenario from `desktop-gpui/probes/<name>.json` through probe.py's own engine
  (`run_scenario`, the step runners, the focus-theft retry, the tally) against your
  instance. Every requirement the scenario checks (all golden queries, all latency
  triggers with their p99, every adapter) is checked; nothing is sampled. Use it for
  anything a feature map entry names a scenario for.
- **`call` is for ad-hoc observation** and for behaviour no scenario covers. A
  hand-written drive samples a requirement, so it is never a substitute for the
  scenario that judges it.

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name shell
echo "exit $?"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name shell --instances 2
```

What `scenario` does:

- **Before the run.** It checks the instance is yours and the build is this checkout's
  clean `HEAD` (exit 2 otherwise: commit, stop, launch again). It compiles probe.py's
  helpers and builds the release binary if a step needs it, before taking the desktop
  lock, as `probe.py run` does. Then it holds the desktop lock and keeps the display
  awake for the whole run, refuses a locked screen (exit 3), and refreshes your lease
  heartbeat at every step.
- **Your instance.** It replaces your instance's process and temp root with a fresh one
  under the same name, binary and claim, because every acceptance scenario starts from
  a fresh instance. It applies the scenario's `setup` (files, graph, fixtures), then
  runs the steps. A `restart` step restarts your instance in place: the data dir is
  kept, fixtures are replayed, and the new pid is written to your claim. When the
  scenario ends, your instance is still running and its claim names the live process.
- **Extra instances.** `--instances N` runs the scenario on your instance plus N-1 extra
  instances (`<instance>-2`, ...), each claimed by you and added to your lease, and adds
  probe.py's side-by-side isolation checks. The extra instances are stopped and their
  claims removed when the command ends. A `release_budget` step starts and stops its own
  release-build instance inside the step.
- **Artifacts, all in `$OUT/<name>/`:**
  - `transcript.txt`: probe.py's report, one PASS / FAIL / ENV-BLOCKED / RETRY line per
    step
  - one `<instance>-NN-<kind>.json` per step run: the step, its verdict, its detail or
    expected/actual, every request and response it exchanged with the app (state, a11y
    trees, metrics, ...) and the values saved so far (budget measurements)
  - screenshots and OCR/pixel captures as `<instance>-NN-...png`
  - `result.json`: scenario, requirement, `sha`, `exit`, `verdict`, counts, per-step
    verdicts, the saved measurements and the artifact list

  The directory is emptied at the start of each run of that scenario.
- **Exit codes:** exactly probe.py's. 0 every step passed; 1 a product FAIL; 2 a
  scenario or harness error; 3 only environment blocks (including focus stolen twice).
  With several `--name`s, an error beats a failure beats a block.
- **Read-only input:** the scenario files are only read. `--name` takes a name, never a
  path.

### Ad-hoc drives with `call`

**One proof is one `call`**, and that includes its side-effect checks. A call:

1. holds probe.py's desktop lock throughout and keeps the display awake (the key window and pasteboard belong to the
   whole desktop; a concurrent `probe.py run` and you wait for each other, up to 15
   minutes)
2. checks you still hold the session lease, and refreshes it after every step
3. starts by closing any open panel (probe.py's `reset_panel`, saved as the call's first
   record)
4. runs each step through probe.py's own runner
5. restores your pasteboard text at the end

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/shell"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  pasteboard:set_nonce \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"window_verified":true,"shell.visible":true,"shell.route":"capture"}' \
  'see:{"role":"Label","text":"Capture"}' \
  a11y screenshot:capture opened 'pasteboard:{"changed":false}'
```

A step is one of:

- **A verb.** `state`, or `verb:JSON-args`: `'open_view:{"route":"browser"}'`,
  `'key:{"keys":"down down enter"}'`, `'set_appearance:{"mode":"dark"}'`,
  `'load_fixture:{"kind":"capture","path":"{fixtures}/capture-ok.json"}'`.
- **`wait_state:{dot.path: value}`.** Polls `state` until every path holds. Values are
  literals or probe.py matchers such as `{"len": 0}`, `{"in": [...]}`, `{"gt": 0}`.
- **`see:QUERY` / `absent:QUERY`.** Polls the accessibility tree (GPUI's AccessKit dump,
  no TCC grant needed) until a node matches, or until none does. QUERY is probe.py's
  `see` query, for example
  `{"role": "ListItem", "text": "beta.md", "deep": true, "state": {"selected": true}}`.
- **`pasteboard:set_nonce`, then `pasteboard:{"expect": ..., "changed": true|false}`.**
  A side effect on the pasteboard, checked inside the same call. `{nonce}` is usable
  after `set_nonce`.
- **`screenshot[:label]`.** Captures the panel window by its `window_id` with
  `screencapture -x -o -l<id>` into `--out`. It needs Screen Recording.
- **`step:{any probe.py step}`.** For example
  `{"call": "resign_key", "expect": {"closed": true}}`, `{"winframe": "gone"}`,
  `{"activate_other": true}` (a real loss of key status to probe.py's own activator
  app), `{"file": "{data_dir}/plugins-state.json", "expect": {...}}`,
  `{"graph": true, "links": [[...]]}` or `{"key_until": {...}}`. `restart`, `process`,
  `release_budget` and `perf_doc` need `probe.py run` itself and are refused.
- **`seed_graph:{"fixture": "graph-backlinks.json"}`.** Setup, not proof: it writes a
  graph fixture into the instance's own `ubilink-graph.sqlite` the way probe.py's
  `setup.graph` does.

Strings may use `{fixtures}`, `{data_dir}`, `{root}` and `{instance}`. Each step prints
one JSON line (`ok`, `detail`, plus `result` for verb calls other than `a11y`). With
`--out`, each step is also saved as `NN-kind.json`, holding every request and response
the step exchanged, plus a line in `transcript.jsonl`. Files are numbered after whatever
is already in that directory.

Every exit path of `call` (and of every harness command) ends in one of these codes; a
Python traceback is a harness bug and exits 2. If the proof cannot start from a closed
panel because the app refuses to close it, that follows probe.py's rule: product failure,
exit 1. Socket trouble during that reset exits 2.

Exit codes:

| exit | meaning |
|---|---|
| 0 | every step held |
| 1 | the product did not do it: an `ok: false` reply, or a check that never held within `--timeout-ms` (default 5000) |
| 2 | harness error: no such instance, socket gone, bad step |
| 3 | environment-blocked: screen locked, display still asleep after the wake, no Screen Recording, the lease is not yours or was reclaimed after idle, or focus stolen twice |

**Proving persistence ("survives a restart").** `call` refuses probe.py's `restart`
step. Prove persistence across three commands instead:

1. a `call` that changes state
2. `restart`
3. a `call` that reads the state back

```sh
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" 'trigger_hotkey:{"action":"settings"}' 'key:{"keys":"space"}' 'step:{"file":"{data_dir}/plugins-state.json","expect":{"bear":true}}'
python3 "$H" restart --instance "$INSTANCE" --verifier "$VERIFIER"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" 'trigger_hotkey:{"action":"settings"}' 'step:{"file":"{data_dir}/plugins-state.json","expect":{"bear":true}}'
```

(The file path and `expect` above show the shape only. Take the real ones from the
feature's map entry.)

What `restart` does, mirroring probe.py's `Instance.restart`:

- It checks the instance is yours and holds the desktop lock. It marks the session lease
  active while it runs.
- It refuses (exit 2) when the binary changed since launch, or when the temp root or
  data dir is gone.
- It quits the app by its recorded pid: SIGTERM, then SIGKILL after 5 seconds, signalled
  only if that pid still runs our binary. It unlinks the stale socket files.
- It keeps the temp root, so the data dir, fixture copy and log survive.
- It relaunches the same binary with the same env: `UBILINK_INSTANCE` (the same socket
  path), `UBILINK_DATA_DIR`, `UBILINK_INSTANCE_SOCKET` and `UBILINK_SKIP_HOTKEYS=1`.
- It waits up to 60 seconds for the socket (`--timeout`).
- It replays every fixture loaded so far. Fixtures live in the app's memory, so the
  claim remembers each `load_fixture` (by kind; a later load of the same kind replaces
  the earlier one).
- It records the new pid in the claim.

It prints `{"RESTARTED": {"old_pid", "new_pid", "restarts", "data_dir_kept",
"fixtures_replayed", ...}}`. Exit codes:

| exit | meaning |
|---|---|
| 0 | relaunched with the same data dir |
| 1 | the product failed: the app refused a fixture replay, or came back reporting another `data_dir` |
| 2 | harness error: the instance did not come back, the binary changed, or the root is gone |
| 3 | not your instance, or the lease is not yours |

Record the before, the restart's output and the after in the same evidence, by passing
all of `$OUT` plus the `restart` output saved to a file. Run the doctor again after a
restart: `process` must now match the new pid.

**Focus theft.** A capture or browser panel hides whenever it loses key status. When a
visibility step fails and another app became frontmost during it (probe.py's
`focus_thief`), the whole call is re-run once from a closed panel (`"attempt": 2`). A
second theft exits 3. A failure with no theft is exit 1.

Use stable handles only:

- route names (`capture`, `browser`, `settings`, `onboarding`)
- hotkey actions (`capture`, `browser`, `settings`)
- a11y roles and their text
- the `state` fields (`route`, `window_verified`, `shell.*`, `view.*`, `data_dir`)
  listed in the README's "Phase 1 view-state contract"

Keyboard input goes through the `key` verb with GPUI key names. Never aim at the screen
by position, and never assume which control the focus lands on next: assert the focused
node with `see:{"role": ..., "state": {"focused": true}}` instead.

The per-feature recipes, with their expected output and proof, are in
`features/README.md` and one file per feature.

## Evidence

- Exercise the real user path. Never internal setters, never test-only endpoints.
- Capture the action *and* the resulting state, not just a final screenshot.
- Verify side effects (rows inserted, files written, messages sent, webhooks fired) alongside what is visible.
- Mocks only where a production boundary already isolates the external system.
- Where the safe path is a dry-run or test mode, verify what it *actually* skips by observing files, network and git refs, not by trusting its name. Some dry-runs still touch the network or open a browser.
- Evidence goes to `.verify/<instance>/<feature-id>/<sha>/`, one directory per feature per head, recorded with `scripts/verify_evidence.py record`.
- Harness output is JSON wherever the harness can emit it, so a verifier can read it without a human.
- Cleanup removes instances, never evidence.

In this app, "the real user path" is the automation verb that runs the user's code path:
`trigger_hotkey` for a hotkey press, `open_view` for the tray's Open, `key` for typing
in the panel. It is never a fixture or `seed_graph` standing in for the behaviour being
proven. Fixtures (`load_fixture`) stand in only where a production boundary already
isolates an external system: the frontmost app being captured, macOS permission state,
SMAppService, file dialogs.

Check side effects inside the same `call` as the action:

- `opened`: the sink of what the app would have opened through the core opener (opens
  never launch anything in an automation build)
- `step:{"file": ...}` / `step:{"graph": ...}`: files under the instance's `{data_dir}`
  (`ubilink-graph.sqlite`, `plugins-state.json`) and fixture files the app rewrites
- `pasteboard:{...}`: the pasteboard

Record every scenario a feature's map entry names in one feature record:

- Copy each scenario's artifacts to a shared staging directory first, renamed with a
  `<scenario>-` prefix. The recorder flattens artifacts by file name, so two scenarios'
  `result.json` and `transcript.txt` would otherwise collide.
- Derive `--result` from every scenario's exit code: `pass` only if all of them exited
  0, otherwise `fail`.

The example is for `shell`, whose map entry names one scenario; list every scenario the
entry names in `SCENARIOS`:

```sh
FEATURE=shell
SCENARIOS="shell"
REC="$WT/.verify-run/$INSTANCE/record-$FEATURE"
rm -rf "$REC"; mkdir -p "$REC"
RESULT=pass
OBSERVED=""
for name in $SCENARIOS; do
  R="$OUT/$name"
  e=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["exit"])' "$R/result.json")
  [ "$e" -eq 0 ] || RESULT=fail
  OBSERVED="$OBSERVED $name: $(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json");"
  for f in "$R"/*; do cp "$f" "$REC/$name-$(basename "$f")"; done
done
python3 "$H" doctor --instance "$INSTANCE"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature "$FEATURE" \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenarios $SCENARIOS (desktop-gpui/probes/*.json) on instance $INSTANCE" \
  --observed "$OBSERVED" \
  --side-effect "as asserted by the scenarios' steps (opened sink, data_dir, files, pasteboard); see each result.json" \
  $(for f in "$REC"/*; do printf -- '--artifact %s ' "$f"; done)
```

Each `result.json`'s `sha` must equal the evidence record's commit. Both come from the
same checkout: the recorder reads `--worktree "$WT"`, and `scenario` reads the same
checkout.

For an ad-hoc `call`, save its exit code right after the drive (`DRIVE_EXIT=$?`) and
derive `--result` from it the same way:

```sh
[ "$DRIVE_EXIT" -eq 0 ] && RESULT=pass || RESULT=fail
python3 "$H" doctor --instance "$INSTANCE"
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature shell \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "trigger_hotkey capture/browser/settings, resign_key and a real deactivation on each, open_view browser, on instance $INSTANCE" \
  --observed "capture and browser: verified popup, blurred, hide on loss of key; settings stays open; tray open_view switches route" \
  --side-effect "opened sink stayed [] (nothing launched); pasteboard unchanged; data_dir is the instance's own temp dir" \
  $(for f in "$OUT"/*; do printf -- '--artifact %s ' "$f"; done)
```

The recorder reads `HEAD` from git in `--worktree` (never from a flag). That is the
skill's checkout, the same one the doctor's `build` check compared the binary against.
It copies each artifact into `.verify/$INSTANCE/shell/<sha>/` and pins it by sha256 in
`evidence.json` (`EVIDENCE: pass <path>`).

- The `for` loop passes every file the drive saved in `$OUT` (each step's JSON,
  screenshots, `transcript.jsonl`). It assumes the path has no spaces; otherwise list
  the files with one `--artifact` each.
- `--side-effect` is required (write `none: <why>` only when there truly is none), and
  so is at least one `--artifact`.
- Artifacts are copied by file name, so two with the same name in one record are
  refused: keep each feature's captures in its own `--out` directory.
- Run the doctor again right before recording if anything (a commit, a rebase) could
  have moved `HEAD` since launch.

## Cleanup

```sh
python3 "$H" stop --instance "$INSTANCE" --verifier "$VERIFIER"
python3 "$H" stop --instance "$INSTANCE_B" --verifier "$VERIFIER"
python3 "$H" list
```

`stop` touches only instances whose claim names you as the verifier; anyone else's exits
3. It reads the claim, checks the recorded pid still runs this instance's binary copy,
sends it SIGTERM, waits up to 5 seconds, then sends SIGKILL (probe.py's `Instance.stop`
semantics). Then it removes:

- the automation socket and its directory
- the temp root (data dir, fixture copy, app log)
- `.verify-run/$INSTANCE/` (claim and binary copy)

It also drops the instance from your session lease, and releases the lease with your
last instance. It prints `{"STOPPED": {..., "still_running": false, "lease": ...}}` and
exits 0, or exits 2 if the process survived. It kills only by the recorded pid, never by
process name, because another owner's instance runs the same binary name. It never
touches `.verify/`: the evidence gate reads those records after cleanup has run.

`list` shows every instance claimed under the run directory, from every verifier sharing
it, with its verifier and whether it is alive, plus the current session lease. Done means
none of yours is listed and the lease no longer names you.
