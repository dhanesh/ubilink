// Satisfies: B1 (live-app capture I/O seam), S4/RT-2 (automation permission probe),
// TN7 (no permission -> U3 degradation). See .manifold/linking-manifesto-app.md
//
// The macOS I/O seam shared by every live adapter. The adapters themselves are now
// DECLARATIVE (see adapters_config.rs) — this module owns only the things that truly
// touch the OS: the `CommandRunner` abstraction, the real `osascript` runner, the
// frontmost-app detection script, and the automation-permission probe. Keeping these
// behind a trait is what lets the whole capture pipeline be unit-tested without a
// desktop; only `OsaScriptRunner` actually shells out.

use crate::adapter::PermissionState;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The I/O seam (functional-core / imperative-shell). Real impl shells out to
/// `osascript`; tests inject canned replies so all logic is verifiable headlessly.
pub trait CommandRunner {
    /// Run a program with args. `Ok(stdout)` on success; `Err(stderr/explanation)`
    /// on a non-zero exit, a spawn failure, or an OS-permission denial.
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

/// The AppleScript that returns the name of the frontmost application — used by the
/// desktop shell to pick which adapter captures the active resource.
pub const FRONTMOST_APP_SCRIPT: &str =
    "tell application \"System Events\" to return name of first application process whose frontmost is true";

/// Interpret an automation-permission PROBE (S4/RT-2) from a command runner. A
/// harmless System Events query: success -> Granted; the TCC "not allowed" error
/// (-1743) -> Denied; anything else (osascript missing, non-mac) -> NotDetermined,
/// so the capture path degrades honestly instead of pretending. Pure + testable.
pub fn probe_automation_permission<R: CommandRunner>(runner: &R) -> PermissionState {
    match runner.run(
        "osascript",
        &[
            "-e",
            "tell application \"System Events\" to return name of first process",
        ],
    ) {
        Ok(_) => PermissionState::Granted,
        Err(e) => {
            // -1743 = errAEEventNotPermitted (user denied automation access).
            if e.contains("-1743") || e.to_lowercase().contains("not allowed") {
                PermissionState::Denied
            } else {
                PermissionState::NotDetermined
            }
        }
    }
}

/// Overall deadline for one `osascript` call (T4). A capture normally answers in
/// well under a second, so this only fires on a stalled or hung target app, which
/// would otherwise block until the ~120 s Apple Event timeout. It is deliberately
/// not shorter: the FIRST capture from an app can raise the macOS Automation (TCC)
/// prompt, and osascript blocks until the user answers it. 15 s leaves time to read
/// and answer that prompt; if it does expire, the timeout message tells the user to
/// answer the prompt and try again (the answer is recorded either way).
pub const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(15);

/// Run `program args` and return its stdout, giving up after `timeout` (T4).
/// Both pipes are drained on their own threads (a large output can't fill a pipe
/// and deadlock the wait); on expiry the child is killed and reaped (no zombie) and
/// the error explains the timeout in words the user can act on (U3).
pub fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    run_bounded_observed(program, args, timeout, |_| {})
}

/// `run_bounded`, reporting the child's pid once spawned (lets tests prove reaping).
fn run_bounded_observed(
    program: &str,
    args: &[&str],
    timeout: Duration,
    on_spawn: impl FnOnce(u32),
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    on_spawn(child.id());
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = match wait_until(&mut child, deadline) {
        Ok(Some(status)) => status,
        waited => {
            let _ = child.kill();
            let _ = child.wait(); // reap: never leave a zombie behind
            return Err(match waited {
                Err(e) => format!("could not wait for {program}: {e}"),
                _ => timed_out(timeout),
            });
        }
    };
    // The child has exited, but a grandchild could still hold a pipe open, so the
    // reads share the same deadline instead of blocking on it.
    let (Some(out), Some(err)) = (collect(&stdout, deadline), collect(&stderr, deadline)) else {
        return Err(timed_out(timeout));
    };
    if status.success() {
        Ok(String::from_utf8_lossy(&out).to_string())
    } else {
        Err(String::from_utf8_lossy(&err).trim().to_string())
    }
}

/// The user-facing timeout explanation (U3). Runners are shared by several
/// callers, so it says "the app" and the caller names which one.
fn timed_out(timeout: Duration) -> String {
    format!(
        "the app didn't respond within {} seconds. If macOS is showing a permission \
         prompt, answer it, then try again.",
        timeout.as_secs_f64()
    )
}

/// Read a pipe to EOF on its own thread; the bytes arrive on the returned channel.
fn drain<P: Read + Send + 'static>(pipe: Option<P>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    rx
}

/// A drained pipe's bytes, or `None` if they didn't arrive by `deadline`.
fn collect(rx: &mpsc::Receiver<Vec<u8>>, deadline: Instant) -> Option<Vec<u8>> {
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(bytes) => Some(bytes),
        Err(mpsc::RecvTimeoutError::Disconnected) => Some(Vec::new()),
        Err(mpsc::RecvTimeoutError::Timeout) => None,
    }
}

/// Poll for exit until `deadline`: `Ok(None)` means still running at the deadline.
/// Backs off from 1 ms to 20 ms so a fast osascript isn't delayed noticeably.
fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        thread::sleep(pause.min(left));
        pause = (pause * 2).min(Duration::from_millis(20));
    }
}

/// The real runner: shells out to `osascript`, bounded by `OSASCRIPT_TIMEOUT` (T4).
/// macOS-only; elsewhere it reports the platform limitation as an Err so callers
/// degrade (U3) instead of pretending.
pub struct OsaScriptRunner;

impl CommandRunner for OsaScriptRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        #[cfg(target_os = "macos")]
        {
            run_bounded(program, args, OSASCRIPT_TIMEOUT)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (program, args);
            Err("live app capture is macOS-only on this build".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRunner(Result<String, String>);
    impl CommandRunner for FakeRunner {
        fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
            self.0.clone()
        }
    }

    #[test]
    fn probe_maps_success_to_granted_and_tcc_denial_to_denied() {
        // S4/RT-2: the permission probe reads the real grant state.
        assert_eq!(
            probe_automation_permission(&FakeRunner(Ok("Finder".into()))),
            PermissionState::Granted
        );
        assert_eq!(
            probe_automation_permission(&FakeRunner(Err(
                "execution error: System Events got an error (-1743)".into()
            ))),
            PermissionState::Denied
        );
        // An unrelated failure must NOT be misread as a denial.
        assert_eq!(
            probe_automation_permission(&FakeRunner(Err("osascript: command not found".into()))),
            PermissionState::NotDetermined
        );
    }

    fn pid_exists(pid: u32) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn a_stalled_command_times_out_and_its_child_is_killed_and_reaped() {
        // T4: a target app that never answers must not hang capture. A real child
        // stands in for a stalled osascript.
        let mut pid = None;
        let start = std::time::Instant::now();
        let err = run_bounded_observed("/bin/sleep", &["5"], Duration::from_millis(300), |p| {
            pid = Some(p)
        })
        .expect_err("a stalled command must time out");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "timed out too late: {elapsed:?}"
        );
        // U3: the timeout reads as an explanation, pointing at a pending prompt.
        assert!(err.contains("didn't respond"), "{err}");
        assert!(err.contains("permission prompt"), "{err}");
        // Killed AND reaped: a zombie would still answer `kill -0`.
        let pid = pid.expect("child was spawned");
        assert!(
            !pid_exists(pid),
            "timed-out child {pid} still in the process table (zombie?)"
        );
    }

    #[test]
    fn a_fast_command_still_returns_its_stdout_and_failures_their_stderr() {
        let t = Duration::from_secs(5);
        assert_eq!(run_bounded("/bin/echo", &["hello"], t).unwrap(), "hello\n");
        assert_eq!(
            run_bounded("/bin/sh", &["-c", "echo nope >&2; exit 3"], t).unwrap_err(),
            "nope"
        );
        assert!(run_bounded("/no/such/program", &[], t)
            .unwrap_err()
            .contains("could not run"));
    }

    #[test]
    fn large_output_on_both_pipes_cannot_deadlock_the_wait() {
        // Far beyond a pipe buffer on stdout AND stderr: only concurrent draining
        // lets the child finish before the deadline.
        let script = "head -c 400000 /dev/zero | tr '\\0' a; \
                      head -c 400000 /dev/zero | tr '\\0' b >&2";
        let out = run_bounded("/bin/sh", &["-c", script], Duration::from_secs(10)).unwrap();
        assert_eq!(out.len(), 400_000);
    }

    #[test]
    fn frontmost_script_targets_system_events() {
        // Regression guard: the routing script must ask System Events for the
        // frontmost app (a corrupted script would route every capture wrong).
        assert!(FRONTMOST_APP_SCRIPT.contains("System Events"));
        assert!(FRONTMOST_APP_SCRIPT.contains("frontmost"));
    }
}
