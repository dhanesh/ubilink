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
`scripts/probe.py`'s engine (same warm locked build, same `Instance` isolation, same
desktop lock, same a11y matchers) that keeps an instance alive between commands, so you
can launch, doctor, drive and record as separate steps. Every harness command prints
JSON on stdout; build and lock chatter goes to stderr. The scenario runner
`python3 scripts/probe.py run <scenario>` stays the regression tool; this skill is how a
verifier proves one head by hand. Scenario and matcher syntax:
`desktop-gpui/probes/README.md`.

## Session variables

Run everything from the repository root (or any directory inside the checkout you are
proving). Set these once per shell:

```sh
SKILL="$(git rev-parse --show-toplevel)/desktop-gpui/.claude/skills/verify-ubilink"
H="$SKILL/scripts/ubilink_instance.py"
INSTANCE=a            # 1-16 chars of [a-z0-9-]; a second instance is b
VERIFIER=verifier-1   # see below
```

`VERIFIER` is who records the proof. Under factory-conductor it is the verifier id the
verifier brief gives you, and it MUST NOT be the owner that wrote the change. Working
alone, use one stable name for the whole session (for example your session's agent id),
never the author of the change. Under factory-conductor also export
`VERIFY_EVIDENCE_DIR=<run root>/.verify` so records from a task worktree land where the
conductor reads them; the harness keeps its run state in the sibling `.verify-run/`.

## Launch

```sh
python3 "$H" launch --instance "$INSTANCE"
python3 "$H" launch --instance b              # a second, independent instance
python3 "$H" list                             # what this checkout has running
```

What `launch` does, in order:

1. Claims `.verify-run/$INSTANCE/instance.json` with an O_EXCL create. A live claim
   exits 3 ("already running"); a stale one (dead pid) is replaced. This claim is how
   two owners are kept apart.
2. Builds the automation binary with probe.py's `build_binary`
   (`cargo build --locked --features automation`, the warm shared target dir under
   `~/Library/Caches/ubilink-probe/target`, held under its target lock) and clones the
   produced executable to `.verify-run/$INSTANCE/ubilink-next-automation`. It prints
   the binary's sha256 and the source commit on stderr. A warm build takes a few
   seconds; a cold one several minutes.
3. Starts the instance with probe.py's `Instance` isolation: its own temp root
   `$TMPDIR/ubprobe-vf-$INSTANCE-*` holding `UBILINK_DATA_DIR=<root>/data` (the app's
   one data-dir resolver, `src/paths.rs`, honours it), its own single-instance socket
   `UBILINK_INSTANCE_SOCKET=<root>/si.sock`, its own copy of `probes/fixtures/`,
   `UBILINK_SKIP_HOTKEYS=1`, and every other `UBILINK_*` variable from your shell
   dropped.

**No network port.** ubilink-next opens no TCP port and has no login, so there is no
port to claim with `verify_evidence.py port` and no auth session to keep apart. The
per-instance endpoint that plays the port's role is the automation socket
`$TMPDIR/ubilink-<uid>/vf-$INSTANCE-<hash>/auto.sock`, where `UBILINK_INSTANCE` is
`vf-$INSTANCE-<6 hex of this checkout's run dir>`, so two worktrees that both name an
instance `a` still get different sockets. Ownership is enforced twice: the O_EXCL claim
above, and the app itself refuses to bind a socket another live process is serving
(`automation/socket.rs`). The per-instance data store is the temp root's `data/`; the
"session" is that data dir plus the fixture copy, nothing else.

**Ready** means the automation socket answered a `state` request. `launch` waits up to
60 seconds for it (`--timeout` to change), then prints `{"INSTANCE": {...}}` with the
pid, socket, data dir, binary sha256 and commit, and exits 0. If the process exits or
the socket never answers within the timeout, it prints the app log's tail, removes the
instance and exits 2: stop waiting and read that log. The instance keeps running after
`launch` returns (it is in its own session) until you clean it up.

Before the first launch on a host, `python3 scripts/probe.py doctor` (read-only) tells
you whether this Mac can host GUI probes at all: Screen Recording for screenshots, an
awake display, swiftc for the Quartz helper, the pinned Rust toolchain.

## Doctor

One read-only check per instance, recorded before the first drive and again after any
failure:

```sh
python3 "$H" doctor --instance "$INSTANCE"
python3 "$SKILL"/scripts/verify_evidence.py doctor --instance "$INSTANCE" --verifier "$VERIFIER" $(python3 "$H" doctor --instance "$INSTANCE" --as-args)
```

The first command prints `{"doctor": {"ok": ..., "checks": {...}, "detail": {...}}}`
and exits 0 when every check passes, 1 when one fails. `--as-args` prints the same
verdict as `--ok|--fail --check name=pass|fail ...` for the recorder, which writes
`.verify/$INSTANCE/doctor/<sha>/doctor.json` (`DOCTOR: ok <path>`). The checks:

| check | passes when |
|---|---|
| `process` | the recorded pid is alive and still runs this instance's binary copy (not a reused pid) |
| `build` | the binary's sha256 is the one built at launch, it was built from the current `HEAD`, and neither then nor now are `desktop-gpui/`, `app/`, `src-tauri/` or `design/` dirty. Evidence is bound to `HEAD`, so a dirty or moved checkout cannot be proven: commit, then relaunch |
| `endpoint` | the instance's automation socket answers `state` and reports this instance's own `data_dir` (the port check, for a socket) |
| `isolation` | `state.shell.hotkeys_skipped` is true and the data dir is inside the instance's temp root, never `~/Library/Application Support` |

The JSON also reports `screen_recording` (needed only for `screenshot` steps) and the
current `route`. There is no `auth` check because the app has no login.

## Drive

Every drive is `call`: one or more steps, sent in order over one socket connection
while holding probe.py's desktop lock (`~/Library/Caches/ubilink-probe/desktop.lock`;
the key window and pasteboard are shared by the whole desktop, so a concurrent
`probe.py run` waits for you and you wait for it, up to 15 minutes).

```sh
OUT="$(git rev-parse --show-toplevel)/.verify-run/$INSTANCE/captures/shell"
python3 "$H" call --instance "$INSTANCE" --out "$OUT" \
  state \
  'trigger_hotkey:{"action":"capture"}' \
  'wait_state:{"window_verified":true,"shell.visible":true,"shell.route":"capture"}' \
  'see:{"role":"Label","text":"Capture"}' \
  'absent:{"text":"Browse links"}' \
  a11y screenshot:capture opened
```

A step is a verb (`state`) or `verb:JSON-args` (`'open_view:{"route":"browser"}'`,
`'key:{"keys":"down down enter"}'`, `'set_appearance:{"mode":"dark"}'`,
`'load_fixture:{"kind":"capture","path":"{fixtures}/capture-ok.json"}'`), or a harness
check:

- `wait_state:{dot.path: value}` polls `state` until every path holds (literals or
  probe.py matchers such as `{"len": 0}`, `{"in": [...]}`, `{"gt": 0}`).
- `see:QUERY` / `absent:QUERY` poll the accessibility tree (GPUI's AccessKit dump, no
  TCC grant needed) until a node matches / no node matches. QUERY is probe.py's `see`
  query: `{"role": "ListItem", "text": "beta.md", "deep": true, "state": {"selected": true}}`.
- `screenshot[:label]` captures the panel window by its `window_id` with
  `screencapture -x -o -l<id>` (needs Screen Recording; exit 3 without it).
- `seed_graph:{"fixture": "graph-backlinks.json"}` is setup, not proof: it writes a
  graph fixture into the instance's own `ubilink-graph.sqlite` the way probe.py's
  `setup.graph` does, before you drive a view that reads the graph.

Strings may use `{fixtures}`, `{data_dir}`, `{root}` and `{instance}`. Each step prints
one JSON line (`ok`, `result` or `error`; a11y trees are summarised on stdout, pass
`--full` to print them) and, with `--out`, is saved as `NN-verb.json` plus
`transcript.jsonl`, numbered after whatever is already in that directory. Exit codes:
0 every step held; 1 the product did not (an `ok: false` reply, or a check that never
held within `--timeout-ms`, default 5000); 2 harness error (no such instance, socket
gone, bad step); 3 environment-blocked (no Screen Recording).

Stable handles only: route names (`capture`, `browser`, `settings`, `onboarding`),
hotkey actions (`capture`, `browser`, `settings`), a11y roles and their text, and the
`state` fields (`route`, `window_verified`, `shell.*`, `view.*`, `data_dir`) listed in
the README's "Phase 1 view-state contract". Keyboard input goes through the `key` verb
with GPUI key names. Never aim at the screen by position, and never assume which control
the focus lands on next: assert the focused node with
`see:{"role": ..., "state": {"focused": true}}` instead.

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

In this app, "the real user path" is the automation verb that runs the user's code path
(`trigger_hotkey` for a hotkey press, `open_view` for the tray's Open, `key` for typing
in the panel), never a fixture or `seed_graph` standing in for the behaviour being
proven. Fixtures (`load_fixture`) stand in only where a production boundary already
isolates an external system: the frontmost app being captured, macOS permission state,
SMAppService, file dialogs. Side effects to observe: the `opened` sink (what the app
would have opened through the core opener; opens never launch anything in an automation
build), files under the instance's `{data_dir}` (`ubilink-graph.sqlite`,
`plugins-state.json`), the pasteboard (`pbpaste`) and fixture files the app rewrites.

Record each proof once the drive passed (or failed: record `--result fail` with what you
saw):

```sh
python3 "$H" doctor --instance "$INSTANCE"
python3 "$SKILL"/scripts/verify_evidence.py record --instance "$INSTANCE" --feature shell \
  --verifier "$VERIFIER" --result pass \
  --action "trigger_hotkey capture, then open_view browser and back, on instance $INSTANCE" \
  --observed "state: window_verified, popup, blurred, accessory, route capture; a11y Label Capture, no Browse links" \
  --side-effect "opened sink stayed [] (nothing launched); data_dir is the instance's own temp dir" \
  $(for f in "$OUT"/*; do printf -- '--artifact %s ' "$f"; done)
```

The recorder reads `HEAD` from git (never from a flag), copies each artifact into
`.verify/$INSTANCE/shell/<sha>/` and pins it by sha256 in `evidence.json`
(`EVIDENCE: pass <path>`). The `for` loop passes every file the drive saved in `$OUT`
(each step's JSON, screenshots, `transcript.jsonl`); it assumes the checkout path has no
spaces, otherwise list the files with one `--artifact` each. `--side-effect` is required (write `none: <why>` only when
there truly is none) and so is at least one `--artifact`. Artifacts are copied by file
name, so two with the same name in one record are refused: keep each feature's
captures in its own `--out` directory. Run the doctor again right before recording if
anything (a commit, a rebase) could have moved `HEAD` since launch: a record must name
the commit the binary was built from.

## Cleanup

```sh
python3 "$H" stop --instance "$INSTANCE"
python3 "$H" stop --instance b
python3 "$H" list
```

`stop` reads the instance's claim, checks the recorded pid still runs this instance's
binary copy, sends it SIGTERM, waits up to 5 seconds, then SIGKILL (probe.py's
`Instance.stop` semantics), and removes its automation socket and socket directory, its
temp root (data dir, fixture copy, app log) and `.verify-run/$INSTANCE/` (claim, binary
copy, captures: by now they are copied into `.verify/`). It prints
`{"STOPPED": {..., "still_running": false}}` and exits 0, or 2 if the process survived.
It kills only by the recorded pid, never by process name, because another owner's
instance runs the same binary name. It never touches `.verify/`: the evidence gate
reads those records after cleanup has run. `list` printing nothing means nothing of
this checkout's is left running.
