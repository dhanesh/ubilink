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

/// The real runner: shells out to `osascript`. macOS-only; elsewhere it reports the
/// platform limitation as an Err so callers degrade (U3) instead of pretending.
pub struct OsaScriptRunner;

impl CommandRunner for OsaScriptRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new(program)
                .args(args)
                .output()
                .map_err(|e| format!("could not run {program}: {e}"))?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).to_string())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
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

    #[test]
    fn frontmost_script_targets_system_events() {
        // Regression guard: the routing script must ask System Events for the
        // frontmost app (a corrupted script would route every capture wrong).
        assert!(FRONTMOST_APP_SCRIPT.contains("System Events"));
        assert!(FRONTMOST_APP_SCRIPT.contains("frontmost"));
    }
}
