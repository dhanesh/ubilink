// Satisfies: T5 (adapter-trait addressing layer), B1 (the API half: obtain a link
// to a resource), S4 (least-priv OS automation permission — request only when
// needed, detect & surface missing state), RT-2 (permission grantable/detectable),
// TN7 (B1 capture depends on S4 permission; absent -> U3 degradation), U3 (never
// silent failure).
// See .manifold/linking-manifesto-app.md
//
// The addressing layer. ALL "get a link to the active resource" / "open a link"
// logic sits behind the `ResourceAdapter` trait (T5), so adding an app or a
// platform is a bounded, testable unit rather than a core rewrite. A `FileAdapter`
// (universal, testable) ships here; live-app macOS adapters (Safari/AppleScript,
// Finder, Obsidian URL scheme) are additional trait impls — their LIVE capture
// needs a real desktop, but the trait, registry, permission gate and open-plan are
// all exercised here.
//
// TN7: capturing a link from another app requires OS automation/accessibility
// permission (S4). `capture_active` checks the permission FIRST and, when it is
// absent, returns a `CaptureOutcome::PermissionRequired` carrying a human
// explanation — the UI surfaces that (U3) instead of failing silently.

use crate::api::LinkPolicy;
use crate::capabilities::Platform;
use crate::live_capture::CommandRunner;
use crate::resource::ResourceIdentity;
use std::path::Path;

/// OS automation/accessibility permission state (S4 / RT-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    /// Granted — capture from other apps is allowed.
    Granted,
    /// The user explicitly denied it.
    Denied,
    /// Not yet requested/decided.
    NotDetermined,
}

/// Detect the current automation permission (S4 / RT-2). On macOS the real check
/// is `AXIsProcessTrusted` / TCC (a follow-up requiring a real desktop + entitlement);
/// until then we report `NotDetermined` so the capture path degrades honestly
/// rather than pretending permission exists.
pub fn detect_automation_permission() -> PermissionState {
    // cfg(macos) real TCC/AX check is ENV-blocked here; default to NotDetermined.
    PermissionState::NotDetermined
}

/// Why a capture could not produce a link — each variant carries a user-facing
/// explanation (U3: never silent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
    /// OS automation permission is missing (S4/TN7). `explanation` + `how_to_fix`
    /// are shown to the user; capture is retried after the user grants it.
    PermissionRequired {
        explanation: String,
        how_to_fix: String,
    },
    /// No adapter supports the active app/resource (degradation, U3).
    Unsupported { explanation: String },
    /// The adapter tried but failed (e.g. the app returned nothing).
    AdapterError { explanation: String },
}

impl CaptureFailure {
    /// The human-readable message the UI shows (U3 — always non-empty).
    pub fn explanation(&self) -> &str {
        match self {
            CaptureFailure::PermissionRequired { explanation, .. } => explanation,
            CaptureFailure::Unsupported { explanation } => explanation,
            CaptureFailure::AdapterError { explanation } => explanation,
        }
    }
}

/// A plan to open a link — program + args. Returned (not executed) so it is
/// testable; `open_uri` validates the link and runs it (see `open_plan_for`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPlan {
    pub program: String,
    pub args: Vec<String>,
}

/// The addressing abstraction (T5). One impl per app/platform.
pub trait ResourceAdapter {
    /// Stable adapter id (e.g. "file", "obsidian", "safari.macos").
    fn id(&self) -> &str;
    /// Does this adapter need OS automation permission to capture? (Files don't;
    /// live-app adapters do — this drives the S4 permission gate.)
    fn needs_permission(&self) -> bool;
    /// Capture a link to the resource this adapter is bound to (B1).
    fn capture(&self) -> Result<ResourceIdentity, CaptureFailure>;
    /// Build a plan to open a link (T5). Executed by `open`.
    fn open_plan(&self, resource: &ResourceIdentity) -> OpenPlan;
}

/// Universal file adapter: links a filesystem path. Needs no automation
/// permission, so it is the cross-platform floor (B1/B4) and is fully testable.
pub struct FileAdapter {
    path: String,
}

impl FileAdapter {
    pub fn new(path: impl Into<String>) -> FileAdapter {
        FileAdapter { path: path.into() }
    }
}

impl ResourceAdapter for FileAdapter {
    fn id(&self) -> &str {
        "file"
    }
    fn needs_permission(&self) -> bool {
        false
    }
    fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
        let p = Path::new(&self.path);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.clone());
        let uri = format!("file://{}", self.path);
        let mut id = ResourceIdentity::new(name, uri).with_path(self.path.clone());
        // Add a content hash when the file is readable (RT-3 robustness).
        if let Ok(bytes) = std::fs::read(p) {
            id = id.with_content(&bytes);
        }
        Ok(id)
    }
    fn open_plan(&self, resource: &ResourceIdentity) -> OpenPlan {
        open_plan_for(Platform::current(), &resource.uri)
    }
}

/// How to open `uri` on `platform`: never through a shell, and never in a position
/// where it can be parsed as an option. Pure (returned, not run), so every
/// platform's command is unit-testable from any host.
///
/// - macOS: `open -- <uri>`. `--` ends option parsing (verified on macOS 27:
///   `open -- -foo` treats `-foo` as a path, while `open -foo` parses options).
/// - Linux: `xdg-open <uri>`. xdg-open rejects `--` as an unknown option, so the
///   URI goes alone; the shared validation already refuses a leading `-`.
/// - Windows: `rundll32 url.dll,FileProtocolHandler <uri>`. Hands the URI to the
///   registered protocol handler with no cmd.exe parsing (the old `cmd /C start`
///   let `&`, `|`, `^` inject commands). Space and `"` are percent-encoded so std's
///   argv quoting never wraps the URI in quotes, which rundll32 would pass through
///   verbatim to the handler.
pub fn open_plan_for(platform: Platform, uri: &str) -> OpenPlan {
    match platform {
        Platform::MacOS => OpenPlan {
            program: "open".into(),
            args: vec!["--".into(), uri.into()],
        },
        Platform::Linux => OpenPlan {
            program: "xdg-open".into(),
            args: vec![uri.into()],
        },
        Platform::Windows => OpenPlan {
            program: "rundll32".into(),
            args: vec![
                "url.dll,FileProtocolHandler".into(),
                uri.replace(' ', "%20").replace('"', "%22"),
            ],
        },
    }
}

/// Open a link: validate it with the shared policy first (defence in depth: a row
/// already in the DB may predate validation or come from an old import), then run
/// the platform plan through the `CommandRunner` seam. Errors are user-facing (U3).
pub fn open_uri<R: CommandRunner>(
    runner: &R,
    policy: &LinkPolicy,
    platform: Platform,
    uri: &str,
) -> Result<(), String> {
    policy
        .validate(uri, "")
        .map_err(|e| format!("Refusing to open this link: {}", e.message()))?;
    let plan = open_plan_for(platform, uri);
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    runner
        .run(&plan.program, &args)
        .map(|_| ())
        .map_err(|e| format!("Couldn't open the link: {e}"))
}

/// Capture a link to the active resource, enforcing the S4 permission gate FIRST
/// (TN7). If the adapter needs permission and it is not Granted, return a
/// `PermissionRequired` failure with a user-facing explanation (U3) — never a
/// silent failure.
pub fn capture_active(
    adapter: &dyn ResourceAdapter,
    permission: PermissionState,
) -> Result<ResourceIdentity, CaptureFailure> {
    if adapter.needs_permission() && permission != PermissionState::Granted {
        let how_to_fix = "Grant ubilink automation/accessibility access in System Settings › Privacy & Security, then try again.".to_string();
        let explanation = match permission {
            PermissionState::Denied => format!(
                "ubilink can't read the active resource from \"{}\" because automation access was denied.",
                adapter.id()
            ),
            _ => format!(
                "ubilink needs your permission to read the active resource from \"{}\".",
                adapter.id()
            ),
        };
        return Err(CaptureFailure::PermissionRequired {
            explanation,
            how_to_fix,
        });
    }
    adapter.capture()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_adapter_captures_uri_name_path_and_hash() {
        // B1 (link addr+name) + RT-3 (multiple identifiers) via a real file.
        let dir = std::env::temp_dir().join(format!("ubilink-adapter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("My Note.md");
        std::fs::write(&f, b"hello").unwrap();
        let a = FileAdapter::new(f.to_string_lossy().to_string());

        let id = a.capture().unwrap();
        assert_eq!(id.name, "My Note.md");
        assert!(id.uri.starts_with("file://"));
        assert_eq!(id.path.as_deref(), Some(f.to_string_lossy().as_ref()));
        assert!(
            id.content_hash.is_some(),
            "RT-3: content hash should be set for a readable file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_adapter_needs_no_permission_and_captures_without_gate() {
        // The universal floor: files are linkable with no automation permission.
        let a = FileAdapter::new("/tmp/whatever.txt");
        assert!(!a.needs_permission());
        // Even with NotDetermined permission, a file capture proceeds.
        let r = capture_active(&a, PermissionState::NotDetermined);
        assert!(r.is_ok(), "file capture must not require permission");
    }

    #[test]
    fn permission_gate_blocks_capture_with_explanation_not_silence() {
        // S4 / TN7 / U3: a live-app adapter without permission yields a
        // PermissionRequired failure carrying a human explanation — never silent.
        struct LiveAppAdapter;
        impl ResourceAdapter for LiveAppAdapter {
            fn id(&self) -> &str {
                "safari.macos"
            }
            fn needs_permission(&self) -> bool {
                true
            }
            fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
                panic!("capture must NOT run when permission gate blocks it");
            }
            fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
                OpenPlan {
                    program: "open".into(),
                    args: vec![r.uri.clone()],
                }
            }
        }
        let a = LiveAppAdapter;
        for state in [PermissionState::NotDetermined, PermissionState::Denied] {
            let err = capture_active(&a, state).unwrap_err();
            match err {
                CaptureFailure::PermissionRequired {
                    ref explanation,
                    ref how_to_fix,
                } => {
                    assert!(!explanation.trim().is_empty(), "U3: empty explanation");
                    assert!(!how_to_fix.trim().is_empty());
                    assert!(explanation.contains("safari.macos"));
                }
                other => panic!("expected PermissionRequired, got {other:?}"),
            }
        }
        // With permission granted, the gate opens (and capture runs -> panics here,
        // proving the gate is what stops it). We assert the gate path only.
        assert!(matches!(
            detect_automation_permission(),
            PermissionState::NotDetermined
        ));
    }

    #[test]
    fn file_adapter_open_plan_on_macos_ends_options_before_the_uri() {
        // `open -foo` parses `-foo` as options; `open -- -foo` treats it as a path
        // (verified on macOS 27). The URI must always follow `--`.
        if cfg!(target_os = "macos") {
            let plan = FileAdapter::new("/x").open_plan(&ResourceIdentity::new("n", "https://x"));
            assert_eq!(plan.program, "open");
            assert_eq!(plan.args, vec!["--", "https://x"]);
        }
    }

    #[test]
    fn macos_and_linux_open_plans_pass_the_uri_as_one_argument() {
        let uri = "https://x/?a=1&b=2";
        let mac = open_plan_for(Platform::MacOS, uri);
        assert_eq!(
            (mac.program.as_str(), mac.args.clone()),
            ("open", vec!["--".to_string(), uri.to_string()])
        );
        // xdg-open refuses `--` as an unknown option, so the URI goes alone (the
        // shared validation already refuses a leading '-').
        let linux = open_plan_for(Platform::Linux, uri);
        assert_eq!(
            (linux.program.as_str(), linux.args),
            ("xdg-open", vec![uri.to_string()])
        );
    }

    #[test]
    fn windows_open_plan_uses_no_shell() {
        // `cmd /C start <uri>` let `&`, `|`, `^`… inject commands. rundll32's
        // FileProtocolHandler hands the URI to the shell's URL handler directly —
        // no cmd.exe parsing anywhere.
        let uri = "https://x/?a=1&b=2|calc^&whoami";
        let plan = open_plan_for(Platform::Windows, uri);
        assert_eq!(plan.program, "rundll32");
        assert_eq!(plan.args, vec!["url.dll,FileProtocolHandler", uri]);
        assert!(!plan.program.eq_ignore_ascii_case("cmd"));
        assert!(!plan.args.iter().any(|a| a.eq_ignore_ascii_case("/c")));
    }

    #[test]
    fn windows_open_plan_percent_encodes_space_and_quote() {
        // Keep the URI a single argv token that std never needs to quote: a space or
        // `"` would make std wrap it in quotes, which rundll32 passes through verbatim.
        let plan = open_plan_for(Platform::Windows, "file:///C:/My Docs/a\"b.txt");
        assert_eq!(plan.args[1], "file:///C:/My%20Docs/a%22b.txt");
    }

    /// Records every command instead of running it (the `CommandRunner` seam).
    struct RecordingRunner {
        calls: std::cell::RefCell<Vec<(String, Vec<String>)>>,
        reply: Result<String, String>,
    }
    impl RecordingRunner {
        fn new(reply: Result<String, String>) -> RecordingRunner {
            RecordingRunner {
                calls: Default::default(),
                reply,
            }
        }
    }
    impl CommandRunner for RecordingRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            self.calls.borrow_mut().push((
                program.to_string(),
                args.iter().map(|a| a.to_string()).collect(),
            ));
            self.reply.clone()
        }
    }

    #[test]
    fn open_uri_runs_the_platform_command_through_the_runner() {
        let policy = LinkPolicy::new();
        let rec = RecordingRunner::new(Ok(String::new()));
        open_uri(&rec, &policy, Platform::Windows, "https://x/?a=1&b=2").unwrap();
        open_uri(
            &rec,
            &policy,
            Platform::MacOS,
            "file:///Users/me/My Note.md",
        )
        .unwrap();
        assert_eq!(
            *rec.calls.borrow(),
            vec![
                (
                    "rundll32".to_string(),
                    vec![
                        "url.dll,FileProtocolHandler".to_string(),
                        "https://x/?a=1&b=2".to_string()
                    ]
                ),
                (
                    "open".to_string(),
                    vec!["--".to_string(), "file:///Users/me/My Note.md".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn open_uri_refuses_invalid_links_without_running_anything() {
        // Defence in depth for rows already in the DB (captured/imported before the
        // policy existed): validate again right before launching.
        let policy = LinkPolicy::new();
        let rec = RecordingRunner::new(Ok(String::new()));
        for uri in [
            "javascript:alert(1)",
            "-a Calculator",
            "smb://evil.example/share",
            "https://x/\u{202E}fdp.exe",
            "",
        ] {
            for platform in [Platform::MacOS, Platform::Windows, Platform::Linux] {
                let err = open_uri(&rec, &policy, platform, uri).unwrap_err();
                assert!(!err.trim().is_empty(), "U3: explained ({uri:?})");
            }
        }
        assert!(rec.calls.borrow().is_empty(), "nothing may be launched");
    }

    #[test]
    fn open_uri_surfaces_a_runner_failure() {
        let rec = RecordingRunner::new(Err("LSOpenURLsWithRole() failed".into()));
        let err = open_uri(&rec, &LinkPolicy::new(), Platform::MacOS, "https://x").unwrap_err();
        assert!(err.contains("LSOpenURLsWithRole"), "{err}");
    }

    #[test]
    fn open_plan_targets_the_resource_uri() {
        let a = FileAdapter::new("/x/y.txt");
        let id = ResourceIdentity::new("y.txt", "obsidian://vault/y");
        let plan = a.open_plan(&id);
        assert!(
            plan.args.iter().any(|arg| arg == "obsidian://vault/y"),
            "open plan must target the resource URI: {plan:?}"
        );
    }
}
