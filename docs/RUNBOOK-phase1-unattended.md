# Runbook: preflight for the unattended GPUI Phase 1 run

Phase 1 (`docs/superpowers/specs/2026-10-03-gpui-phase1-port.md`) runs under
factory-conductor with nobody watching. Its proofs drive a real GUI: they open the
panel, read the accessibility tree, take screenshots, write the pasteboard. A Mac that
locks, sleeps, shows a dialog or lets another app grab focus does not make the product
fail, but it does make the proofs ENV-BLOCKED, and a run full of exit 3s proves
nothing. Do every step below before starting the conductor. (Final review I10.)

## 1. Keep the Mac awake for the whole run

Start the conductor under `caffeinate`, so neither the display nor the system sleeps
while it runs, whatever the energy settings say:

```sh
caffeinate -dimsu <the conductor command>
```

`-d` display, `-i` idle sleep, `-m` disk, `-s` system sleep on AC, `-u` declares user
activity (it also wakes a display that is already off). `probe.py run` holds its own
`caffeinate -d -i -u` per run, but the gaps between runs are the conductor's.

Plug the Mac into power: `-s` only holds on AC.

## 2. No screen lock, no screen saver

A locked screen keeps every panel from becoming key, so every a11y read fails. The
harness reports that as ENV-BLOCKED ("the screen is locked"), and nothing can be
proven until someone unlocks it.

- System Settings > Lock Screen: "Start Screen Saver when inactive" Never, "Turn
  display off when inactive" Never, "Require password after screen saver begins or
  display is turned off" Never (or the longest value, if policy forbids Never).
- Do not lock the screen by hand (Ctrl-Cmd-Q, hot corners) while the run is going.
- Turn on a Focus mode (Do Not Disturb) so notification banners do not cover the panel
  or take focus.

## 3. Grants for the host terminal

Probes run as children of the terminal (or agent host) that starts the conductor, and
macOS attributes their TCC requests to it:

- **Screen Recording** (System Settings > Privacy & Security > Screen Recording): for
  `ocr`, `pixel` and screenshots. Without it those steps are ENV-BLOCKED.
- **Automation > System Events**: the release-build instance that `probe.py run
  performance` starts runs the app's one startup automation-permission probe
  (`desktop-gpui/src/app_state.rs`; automation builds skip it, I5). Grant it once
  ahead of the run, or a TCC prompt can sit on screen during the run.
- **Accessibility** is not needed by the probes (they read the app's AccessKit tree
  over the automation socket).

## 4. Preflight must pass

```sh
python3 scripts/probe.py doctor
```

It checks Screen Recording, an awake display, the display count, the OCR helper,
`caffeinate`, Xcode's `swiftc` and the pinned Rust toolchain. Every check must PASS
(`--build` also runs the locked automation build). Fix anything it reports before
going on.

## 5. One warm build cache, shared by every clone

The conductor runs each verify command in a fresh `git clone` with a 600 s limit per
command (`VERIFY_TIMEOUT`). A cold GPUI build alone takes longer. Export one shared
target directory in the conductor's environment (ruling R-ad); `probe.py` (`run`,
`release-clean`, `cargo-test`), the verify harness and `make verify` all build there:

```sh
export CARGO_TARGET_DIR="$HOME/Library/Caches/ubilink-probe/target"
```

Warm it once from this checkout, with the same variable exported, before the run:

```sh
python3 scripts/probe.py run shell          # the automation debug build + helpers
python3 scripts/probe.py release-clean      # both release builds
python3 scripts/probe.py cargo-test router_registers_four_views
make verify                                 # every crate's test and clippy profile
```

A fresh clone still recompiles the path crates (their source path differs), which
is what the measured timings in
`.superpowers/sdd/2026-10-03-gpui-phase0-plan/final-fix-report.md` include.

## 6. Nothing else on the desktop

- Quit apps that pop windows or take focus on their own: mail and chat clients,
  updaters, IDEs that steal focus on build, video calls, password managers' prompts.
- Quit any running `ubilink` (`pgrep -x ubilink` prints nothing): an installed
  Ubilink.app owns the global hotkeys and the default single-instance socket.
- Do not use the Mac while the run goes. The pasteboard, the key window and the cursor
  are shared, and a click is a focus theft.

## 7. Build cache, disk and environment

The shared target dir in step 5 only works if there is room for it and nothing
contaminates it. Do this before the run, in the same shell the conductor's agent
host will run in:

```sh
# need >= 15 GB free first. Safe to free once CARGO_TARGET_DIR is set: the per-crate target dirs
#   rm -rf src-tauri/target registry/target app/target desktop-gpui/target
# Never seed or merge the shared dir by copying other target dirs (that caused E0463).
export CARGO_TARGET_DIR="$HOME/Library/Caches/ubilink-probe/target"
export CARGO_INCREMENTAL=0
# Export BEFORE launching the agent host: conductor.py is a per-subcommand CLI and its verify
# commands inherit the host's environment.
caffeinate -dimsu claude
# Warm once, same env:
python3 scripts/probe.py run shell
python3 scripts/probe.py release-clean
python3 scripts/probe.py cargo-test router_registers_four_views
make verify
# Between tasks, with no cargo running (path-crate artifacts are clone-specific and never reused):
rm -rf "$CARGO_TARGET_DIR"/{debug,release}/incremental
find "$CARGO_TARGET_DIR"/{debug,release} -maxdepth 2 -name '*ubilink*' -exec rm -rf {} +
```

Check free disk space first: the shared target dir needs at least 15 GB free before
it is warmed. Once `CARGO_TARGET_DIR` is exported, the old per-crate target dirs
(`src-tauri/target`, `registry/target`, `app/target`, `desktop-gpui/target`) are no longer used and are safe to remove to make room. Never
seed or merge the shared dir by copying in another target dir's contents — mixing
build metadata from different target dirs this way is what caused E0463 in an
earlier run.

`CARGO_TARGET_DIR` and `CARGO_INCREMENTAL=0` must be exported **before** the agent
host (the `claude` process under `caffeinate` above) is launched, not just before
the conductor: `conductor.py` is a per-subcommand CLI, invoked fresh for each verify
command, and each invocation inherits whatever environment the host process already
has — it cannot pick up variables exported only inside a subshell started after the
host.

Warm the cache once, in that same environment, with the commands above
(`probe.py run shell`, `probe.py release-clean`, `probe.py cargo-test
router_registers_four_views`, `make verify`) before handing control to the
conductor. Between tasks, with no cargo process running, clear incremental
artifacts and any `*ubilink*` path-crate output under the shared target dir — those
path-crate artifacts are clone-specific (built from a fresh clone's path) and are
never reused by a later clone, so leaving them only wastes disk.

Builds in this one shared target dir serialise on cargo's own lock, and GUI proofs
serialise on the desktop lock (SKILL.md's session lease), so `max_parallel: 1` is
the recommended conductor setting; `max_parallel: 2` only adds lock-wait time inside
each verify command's 600 s limit, it does not make two builds or two GUI proofs run
at once. An ENV-BLOCKED (exit 3) verify is still counted by the conductor as a
failed verify and spends one of the task's repair attempts, which is exactly why
this whole preflight matters — an avoidable environment block costs a repair the
same way a real product bug would. Measure `python3 scripts/probe.py run
performance` once, before the unattended run starts, so its timing is in hand ahead
of time rather than discovered mid-run against the per-command limit.

## What the run's exit codes mean

| Exit | Meaning | What to do |
|---|---|---|
| 0 | PASS | nothing |
| 1 | the product did not do what a step expects | a real failure: the task's work is wrong |
| 2 | harness or scenario error (build failure, an instance that never served its socket) | never the product's verdict; look at the log |
| 3 | ENV-BLOCKED: the host could not run a step | fix the environment and re-run; never counted as the product failing |

ENV-BLOCKED causes, and the fix for each: missing Screen Recording (step 3), display
asleep or screen locked (steps 1-2), focus taken twice by another app (step 6), too
few displays for the multi-display probe, `swiftc` missing (step 4), and "desktop
busy" / "build cache busy": another probe held the desktop or the build cache past
the 480 s wait. GUI proofs serialise on one desktop, so parallel tasks only overlap
their builds; a second verifier's harness `launch` gives up on the session lease
after 300 s (also exit 3). Once a step is ENV-BLOCKED, the rest of that instance's
steps are reported SKIPPED, not run, so an environment problem never becomes a
product FAIL.

## After a killed run

A verify command killed at the 600 s limit cannot tear down its instances. The next
`probe.py run` (or harness `launch`) reaps them: an instance whose owning probe is
dead is terminated after checking its pid still runs the recorded binary. When the
whole run is over, check that nothing is left:

```sh
pgrep -x ubilink || echo "nothing running"
```
