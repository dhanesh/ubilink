//! The launch-at-login seam (Phase 1 R13, spec D6; Task 11 ruling R-o).
//!
//! The settings view's launch-at-login switch is backed by `SMAppService.mainApp`.
//! Probes must never register a real login item on the host (O4), and the unbundled
//! binary a probe runs could not register one anyway, so every status read,
//! register and unregister asks this seam first, the same way file sources ask
//! [`crate::fixtures::path`]:
//!
//! - [`status`], [`register`] and [`unregister`] return `Some(..)` in every
//!   `automation` build: the fixture's result when a `login_item` fixture is loaded,
//!   else `Some(Err(..))` saying none is (final review I9: falling through to the
//!   real `SMAppService` there would let a probe touch the host's login items, O4).
//!   The view then shows that result and must not touch `SMAppService`.
//! - `None` (always, and only, in a release build) means "no seam": the view calls
//!   `SMAppService.mainApp` itself (Phase 1's job; this module never does).
//!
//! The fixture file is this instance's login-item store: a register or unregister
//! rewrites its `status` and appends the call to its `calls` list, so the status
//! survives an app restart (as the real one does) and a probe can read back which
//! calls the view made (`file` step on the fixture copy). Its shape is documented in
//! `desktop-gpui/probes/fixtures/README.md` (`login-item-*.json`).
//!
//! Compiled in every build so Phase 1's view needs no `cfg`; the fixture logic is
//! automation-only and the kind name lives with the automation protocol (TN1).

// Phase 1's settings view is the caller; until then rustc sees no use.
#![allow(dead_code)]

/// `SMAppService.Status`, as the settings view must show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
}

impl Status {
    /// The status label's exact text (probes/README.md "Phase 1 view-state contract").
    pub fn label(self) -> &'static str {
        match self {
            Status::NotRegistered => "Not registered",
            Status::Enabled => "Enabled",
            Status::RequiresApproval => "Requires approval",
            Status::NotFound => "Not found",
        }
    }

    /// Whether the switch shows on: the app is registered (approved or awaiting it).
    pub fn is_on(self) -> bool {
        matches!(self, Status::Enabled | Status::RequiresApproval)
    }

    #[cfg(any(feature = "automation", test))]
    fn key(self) -> &'static str {
        match self {
            Status::NotRegistered => "not_registered",
            Status::Enabled => "enabled",
            Status::RequiresApproval => "requires_approval",
            Status::NotFound => "not_found",
        }
    }

    #[cfg(any(feature = "automation", test))]
    fn from_key(key: &str) -> Option<Status> {
        [
            Status::NotRegistered,
            Status::Enabled,
            Status::RequiresApproval,
            Status::NotFound,
        ]
        .into_iter()
        .find(|s| s.key() == key)
    }
}

/// What a status read, register or unregister reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    /// For requires-approval and not-found: the sentence the view shows with the
    /// status, saying what it means and what to do (U3).
    pub explanation: Option<String>,
}

/// The current status, from the seam; `None` = ask `SMAppService.mainApp.status`.
pub fn status() -> Option<Result<Report, String>> {
    seam(Op::Status)
}

/// Register the app as a login item, through the seam; `None` = call
/// `SMAppService.mainApp.register()`.
pub fn register() -> Option<Result<Report, String>> {
    seam(Op::Register)
}

/// Unregister it, through the seam; `None` = call `SMAppService.mainApp.unregister()`.
pub fn unregister() -> Option<Result<Report, String>> {
    seam(Op::Unregister)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Status,
    Register,
    Unregister,
}

/// The error an automation build reports when no `login_item` fixture is loaded.
#[cfg(any(feature = "automation", test))]
pub const NO_FIXTURE: &str = "no login_item fixture loaded";

/// Final review I9 (O4): an automation build never falls through to the real
/// `SMAppService`. Without a fixture it answers with an error the view shows (U3),
/// so a probe that forgot `load_fixture` sees a visible failure, not a real login
/// item registered on the host.
#[cfg(feature = "automation")]
fn seam(op: Op) -> Option<Result<Report, String>> {
    Some(seam_with(
        crate::fixtures::path(crate::fixtures::Fixture::LoginItem),
        op,
    ))
}

#[cfg(feature = "automation")]
fn seam_with(fixture: Option<std::path::PathBuf>, op: Op) -> Result<Report, String> {
    match fixture {
        Some(path) => run_on_file(&path, op),
        None => Err(NO_FIXTURE.into()),
    }
}

#[cfg(not(feature = "automation"))]
fn seam(_op: Op) -> Option<Result<Report, String>> {
    None
}

#[cfg(feature = "automation")]
fn run_on_file(path: &std::path::Path, op: Op) -> Result<Report, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("login_item fixture {}: {e}", path.display()))?;
    let mut doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("login_item fixture {}: {e}", path.display()))?;
    let report = apply(&mut doc, op)?;
    if op != Op::Status {
        std::fs::write(path, doc.to_string())
            .map_err(|e| format!("login_item fixture {}: {e}", path.display()))?;
    }
    Ok(report)
}

/// The seam's state machine over the fixture document: `status` is the current
/// status; register moves it to `on_register`, unregister to not-registered; both
/// append their name to `calls`. `explanation` is reported with requires-approval
/// and not-found.
#[cfg(any(feature = "automation", test))]
fn apply(doc: &mut serde_json::Value, op: Op) -> Result<Report, String> {
    let obj = doc
        .as_object_mut()
        .ok_or("login_item fixture: expected a JSON object")?;
    let read = |obj: &serde_json::Map<String, serde_json::Value>, key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .and_then(Status::from_key)
            .ok_or_else(|| format!("login_item fixture: bad or missing {key:?}"))
    };
    let current = read(obj, "status")?;
    let next = match op {
        Op::Status => current,
        Op::Register => read(obj, "on_register")?,
        Op::Unregister => Status::NotRegistered,
    };
    if op != Op::Status {
        obj.insert("status".into(), next.key().into());
        let call = if op == Op::Register {
            "register"
        } else {
            "unregister"
        };
        let calls = obj
            .entry("calls")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        match calls.as_array_mut() {
            Some(list) => list.push(call.into()),
            None => return Err("login_item fixture: calls must be a list".into()),
        }
    }
    let explanation = matches!(next, Status::RequiresApproval | Status::NotFound)
        .then(|| {
            obj.get("explanation")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .flatten();
    Ok(Report {
        status: next,
        explanation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn register_and_unregister_walk_the_fixture_and_record_calls() {
        let mut doc = json!({"status": "not_registered", "on_register": "requires_approval",
                             "explanation": "Approve it in System Settings > General > Login Items."});
        let r = apply(&mut doc, Op::Status).unwrap();
        assert_eq!((r.status, r.explanation), (Status::NotRegistered, None));
        let r = apply(&mut doc, Op::Register).unwrap();
        assert_eq!(r.status, Status::RequiresApproval);
        assert!(r.explanation.unwrap().contains("Login Items"));
        assert_eq!(doc["status"], "requires_approval");
        assert_eq!(
            apply(&mut doc, Op::Status).unwrap().status,
            Status::RequiresApproval
        );
        let r = apply(&mut doc, Op::Unregister).unwrap();
        assert_eq!((r.status, r.explanation), (Status::NotRegistered, None));
        assert_eq!(doc["calls"], json!(["register", "unregister"]));
    }

    #[test]
    fn a_bad_fixture_is_an_error_not_a_status() {
        assert!(apply(&mut json!([]), Op::Status).is_err());
        assert!(apply(&mut json!({"status": "maybe"}), Op::Status).is_err());
        assert!(apply(&mut json!({"status": "not_registered"}), Op::Register).is_err());
    }

    #[test]
    fn labels_and_switch_state() {
        assert_eq!(Status::RequiresApproval.label(), "Requires approval");
        assert_eq!(Status::NotFound.label(), "Not found");
        assert!(Status::Enabled.is_on() && Status::RequiresApproval.is_on());
        assert!(!Status::NotFound.is_on() && !Status::NotRegistered.is_on());
    }

    /// I9: with no fixture loaded an automation build answers every call with an
    /// error, never `None` (which would send the view to the real SMAppService).
    #[cfg(feature = "automation")]
    #[test]
    fn automation_builds_without_a_fixture_fail_closed() {
        for op in [Op::Status, Op::Register, Op::Unregister] {
            assert_eq!(seam_with(None, op), Err(NO_FIXTURE.to_string()));
        }
        // Through the public calls too: this test binary loads no fixture.
        assert_eq!(status(), Some(Err(NO_FIXTURE.to_string())));
        assert_eq!(register(), Some(Err(NO_FIXTURE.to_string())));
        assert_eq!(unregister(), Some(Err(NO_FIXTURE.to_string())));
    }

    #[cfg(not(feature = "automation"))]
    #[test]
    fn release_builds_have_no_seam() {
        assert_eq!(status(), None);
        assert_eq!(register(), None);
    }
}
