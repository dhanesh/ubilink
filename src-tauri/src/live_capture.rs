// Satisfies: B1 (live-app half of "get a link to the active resource"), S4/RT-2
// (capture from another app is automation-permission-gated; probe detects the
// grant), TN7 (no permission -> U3 degradation, never silent), U3 (every failure
// carries a human explanation), T5 (one bounded ResourceAdapter impl per app).
// See .manifold/linking-manifesto-app.md
//
// Live macOS app capture: Safari / Chrome / Finder. The actual product surface —
// "hook the thing I'm looking at right now" — for apps that expose their active
// resource over AppleScript.
//
// ENV boundary (why this used to be wholly ENV-blocked, and what is NOT anymore):
// the ONLY part that needs a real desktop + a live app + a TCC automation grant is
// the single `osascript` invocation. Everything else — which script to send, how to
// parse the reply into a ResourceIdentity, how an empty/forbidden reply degrades
// (U3), and how the permission probe maps an osascript error to Granted/Denied — is
// pure logic behind the `CommandRunner` seam and is fully unit-tested here with a
// fake runner. The real `OsaScriptRunner` is the only thing the manual runbook
// (docs/RUNBOOK-live-capture.md) has to confirm on a Mac.

use crate::adapter::{CaptureFailure, OpenPlan, PermissionState, ResourceAdapter};
use crate::resource::ResourceIdentity;

/// The I/O seam (functional-core / imperative-shell). Real impl shells out to
/// `osascript`; tests inject canned replies so all logic is verifiable headlessly.
pub trait CommandRunner {
    /// Run a program with args. `Ok(stdout)` on success; `Err(stderr/explanation)`
    /// on a non-zero exit, a spawn failure, or an OS-permission denial.
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

/// Which app we are capturing from — selects the AppleScript and the URI shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppKind {
    Safari,
    Chrome,
    Finder,
}

impl AppKind {
    fn adapter_id(self) -> &'static str {
        match self {
            AppKind::Safari => "safari.macos",
            AppKind::Chrome => "chrome.macos",
            AppKind::Finder => "finder.macos",
        }
    }

    /// The AppleScript that asks the app for its active resource. Browsers return
    /// `url<TAB>title`; Finder returns the POSIX path of the selected item.
    fn script(self) -> &'static str {
        match self {
            AppKind::Safari =>
                "tell application \"Safari\" to return (URL of current tab of front window) \
                 & \"\\t\" & (name of current tab of front window)",
            AppKind::Chrome =>
                "tell application \"Google Chrome\" to return (URL of active tab of front window) \
                 & \"\\t\" & (title of active tab of front window)",
            AppKind::Finder =>
                "tell application \"Finder\" to return POSIX path of (target of front window as alias)",
        }
    }
}

/// A macOS live-app adapter, generic over the command runner so the logic is
/// testable without a desktop.
pub struct MacAppAdapter<R: CommandRunner> {
    kind: AppKind,
    runner: R,
}

impl<R: CommandRunner> MacAppAdapter<R> {
    pub fn new(kind: AppKind, runner: R) -> MacAppAdapter<R> {
        MacAppAdapter { kind, runner }
    }

    /// Turn the runner's raw stdout into a ResourceIdentity. Pure: no I/O.
    fn parse(&self, raw: &str) -> Result<ResourceIdentity, CaptureFailure> {
        // Take the first line and strip only its line endings — NOT a leading tab,
        // which is a meaningful empty-URL field for the browser case.
        let line = raw.lines().next().unwrap_or("");
        if line.trim().is_empty() {
            return Err(CaptureFailure::AdapterError {
                explanation: format!(
                    "{} returned no active resource — open a window/tab and try again.",
                    self.kind.adapter_id()
                ),
            });
        }
        match self.kind {
            AppKind::Finder => {
                // A filesystem path -> a file:// link (consistent with FileAdapter).
                let path = line.trim().to_string();
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                let uri = format!("file://{path}");
                Ok(ResourceIdentity::new(name, uri).with_path(path))
            }
            AppKind::Safari | AppKind::Chrome => {
                // `url \t title`. Title may be empty; URL must not be.
                let (url, title) = match line.split_once('\t') {
                    Some((u, t)) => (u.trim(), t.trim()),
                    None => (line.trim(), ""),
                };
                if url.is_empty() {
                    return Err(CaptureFailure::AdapterError {
                        explanation: format!(
                            "{} returned a title but no URL — nothing to link.",
                            self.kind.adapter_id()
                        ),
                    });
                }
                let name = if title.is_empty() { url.to_string() } else { title.to_string() };
                Ok(ResourceIdentity::new(name, url.to_string()))
            }
        }
    }
}

impl<R: CommandRunner> ResourceAdapter for MacAppAdapter<R> {
    fn id(&self) -> &str {
        self.kind.adapter_id()
    }
    fn needs_permission(&self) -> bool {
        // Reading another app's state over AppleScript requires the automation grant.
        true
    }
    fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
        // The single ENV-blocked syscall. A denial here (TCC) surfaces as an
        // AdapterError explanation (U3) rather than a panic; the S4 gate in
        // `capture_active` normally catches a missing grant before we reach this.
        match self.runner.run("osascript", &["-e", self.kind.script()]) {
            Ok(out) => self.parse(&out),
            Err(e) => Err(CaptureFailure::AdapterError {
                explanation: format!(
                    "Couldn't read the active resource from {}: {e}",
                    self.kind.adapter_id()
                ),
            }),
        }
    }
    fn open_plan(&self, resource: &ResourceIdentity) -> OpenPlan {
        // `open` resolves both http(s) URLs and file:// paths on macOS.
        OpenPlan { program: "open".into(), args: vec![resource.uri.clone()] }
    }
}

/// Interpret an automation-permission PROBE (S4/RT-2) from a command runner. A
/// harmless System Events query: success -> Granted; the TCC "not allowed" error
/// (-1743) -> Denied; anything else (osascript missing, non-mac) -> NotDetermined,
/// so the capture path degrades honestly instead of pretending. Pure + testable.
pub fn probe_automation_permission<R: CommandRunner>(runner: &R) -> PermissionState {
    match runner.run(
        "osascript",
        &["-e", "tell application \"System Events\" to return name of first process"],
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
    use crate::adapter::capture_active;

    /// Canned runner: returns a fixed Ok/Err regardless of input (the ENV seam).
    struct FakeRunner(Result<String, String>);
    impl CommandRunner for FakeRunner {
        fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
            self.0.clone()
        }
    }

    #[test]
    fn safari_capture_parses_url_and_title() {
        // B1: the active browser resource becomes a link (uri=URL, name=title).
        let a = MacAppAdapter::new(
            AppKind::Safari,
            FakeRunner(Ok("https://example.com/x\tExample Domain\n".into())),
        );
        let id = a.capture().unwrap();
        assert_eq!(id.uri, "https://example.com/x");
        assert_eq!(id.name, "Example Domain");
    }

    #[test]
    fn chrome_capture_parses_active_tab() {
        let a = MacAppAdapter::new(
            AppKind::Chrome,
            FakeRunner(Ok("https://docs.rs/serde\tserde - Rust".into())),
        );
        let id = a.capture().unwrap();
        assert_eq!(id.uri, "https://docs.rs/serde");
        assert_eq!(id.name, "serde - Rust");
    }

    #[test]
    fn finder_capture_builds_file_uri_and_name_from_path() {
        // Finder selection -> file:// link with a path identifier (RT-3).
        let a = MacAppAdapter::new(
            AppKind::Finder,
            FakeRunner(Ok("/Users/me/Docs/Report.pdf\n".into())),
        );
        let id = a.capture().unwrap();
        assert_eq!(id.uri, "file:///Users/me/Docs/Report.pdf");
        assert_eq!(id.name, "Report.pdf");
        assert_eq!(id.path.as_deref(), Some("/Users/me/Docs/Report.pdf"));
    }

    #[test]
    fn empty_reply_degrades_to_explained_error_not_panic() {
        // U3: an app with no active resource explains itself; never silent/panic.
        let a = MacAppAdapter::new(AppKind::Safari, FakeRunner(Ok("   \n".into())));
        match a.capture().unwrap_err() {
            CaptureFailure::AdapterError { explanation } => {
                assert!(!explanation.trim().is_empty());
                assert!(explanation.contains("safari.macos"));
            }
            other => panic!("expected AdapterError, got {other:?}"),
        }
    }

    #[test]
    fn title_only_no_url_is_rejected_with_explanation() {
        // A reply with an empty URL field is not a linkable resource (U3).
        let a = MacAppAdapter::new(AppKind::Chrome, FakeRunner(Ok("\tUntitled".into())));
        assert!(matches!(a.capture(), Err(CaptureFailure::AdapterError { .. })));
    }

    #[test]
    fn runner_error_becomes_adapter_error_with_explanation() {
        // A TCC denial / osascript failure at capture time surfaces (U3), not panic.
        let a = MacAppAdapter::new(
            AppKind::Safari,
            FakeRunner(Err("execution error: Not authorized (-1743)".into())),
        );
        match a.capture().unwrap_err() {
            CaptureFailure::AdapterError { explanation } => {
                assert!(explanation.contains("-1743"));
                assert!(explanation.contains("safari.macos"));
            }
            other => panic!("expected AdapterError, got {other:?}"),
        }
    }

    #[test]
    fn live_adapter_is_permission_gated_before_any_capture() {
        // S4/TN7: with no grant, the gate blocks BEFORE the runner is ever consulted.
        struct ExplodingRunner;
        impl CommandRunner for ExplodingRunner {
            fn run(&self, _p: &str, _a: &[&str]) -> Result<String, String> {
                panic!("runner must not run when the S4 gate blocks capture");
            }
        }
        let a = MacAppAdapter::new(AppKind::Safari, ExplodingRunner);
        assert!(a.needs_permission());
        let err = capture_active(&a, PermissionState::NotDetermined).unwrap_err();
        assert!(matches!(err, CaptureFailure::PermissionRequired { .. }));
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
    fn scripts_name_their_target_app() {
        // Regression guard: a corrupted template that talks to the wrong app would
        // silently capture the wrong window. Pin the app name into each script.
        assert!(AppKind::Safari.script().contains("Safari"));
        assert!(AppKind::Chrome.script().contains("Google Chrome"));
        assert!(AppKind::Finder.script().contains("Finder"));
    }

    #[test]
    fn open_plan_targets_the_resource_uri() {
        let a = MacAppAdapter::new(AppKind::Safari, FakeRunner(Ok("x".into())));
        let id = ResourceIdentity::new("t", "https://example.com");
        let plan = a.open_plan(&id);
        assert_eq!(plan.program, "open");
        assert!(plan.args.iter().any(|x| x == "https://example.com"));
    }

    // ── The irreducible ENV-blocked seam (see docs/RUNBOOK-live-capture.md) ──
    // Ignored by default: it shells out to the REAL osascript and needs a Mac with
    // a TCC automation grant and Safari open to a page. Run on a desktop with:
    //   cargo test -p hooky-core -- --ignored live_capture_against_real_safari
    // Everything ELSE in this module is proven headlessly above; this is the one
    // line the manual runbook confirms.
    #[test]
    #[ignore = "ENV-blocked: needs a real macOS desktop, TCC automation grant, and Safari open"]
    fn live_capture_against_real_safari() {
        let safari = MacAppAdapter::new(AppKind::Safari, OsaScriptRunner);
        let granted = probe_automation_permission(&OsaScriptRunner) == PermissionState::Granted;
        assert!(granted, "grant Automation access to the test runner in System Settings first");
        let id = capture_active(&safari, PermissionState::Granted)
            .expect("capture failed — is Safari open to a page?");
        eprintln!("captured live: uri={} name={}", id.uri, id.name);
        assert!(id.uri.contains("://"), "expected a real URL, got {:?}", id.uri);
    }
}
