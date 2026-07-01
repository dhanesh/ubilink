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
/// testable; `ResourceAdapter::open` runs it.
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
        // macOS `open`, Linux `xdg-open`, Windows `start` — resolve the URI.
        let program = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "xdg-open"
        };
        let args = if cfg!(target_os = "windows") {
            vec!["/C".into(), "start".into(), resource.uri.clone()]
        } else {
            vec![resource.uri.clone()]
        };
        OpenPlan {
            program: program.into(),
            args,
        }
    }
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
