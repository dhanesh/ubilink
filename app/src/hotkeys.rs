//! Defect 1/2's hotkey-report machinery. `HotkeyReport` and `build_hotkey_report`
//! are pure data/folding — no `tauri_plugin_global_shortcut` type appears, so they
//! moved here. `register_hotkeys` (which DOES touch `GlobalShortcutExt`) and
//! `hotkey_shortcuts` (which constructs `tauri_plugin_global_shortcut::Shortcut`
//! values) stay in `desktop/src/main.rs` — see that file's comments.

use serde::Serialize;

use crate::AppState;

/// Defect 1: a failed hotkey registration used to be invisible (only `eprintln!`'d,
/// which a packaged .app has no terminal to show). This is the report the frontend
/// can actually query — per shortcut, whether it registered, plus a human-readable
/// error for the FIRST failure (the important one — an app with 3 dead shortcuts and
/// one already-explained cause doesn't need three redundant error strings).
#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct HotkeyReport {
    pub toggle: bool,
    pub browse: bool,
    pub settings: bool,
    pub error: Option<String>,
}

/// Pure: fold three registration attempts into one `HotkeyReport`. Kept free of any
/// Tauri/global-shortcut types so it's unit-testable without spinning up the plugin.
pub fn build_hotkey_report(
    toggle: Result<(), String>,
    browse: Result<(), String>,
    settings: Result<(), String>,
) -> HotkeyReport {
    let mut error = None;
    for (label, result) in [
        ("Cmd+Shift+K", &toggle),
        ("Cmd+Shift+L", &browse),
        ("Cmd+Shift+,", &settings),
    ] {
        if error.is_none() {
            if let Err(e) = result {
                error = Some(format!("{label}: {e}"));
            }
        }
    }
    HotkeyReport {
        toggle: toggle.is_ok(),
        browse: browse.is_ok(),
        settings: settings.is_ok(),
        error,
    }
}

/// Current hotkey registration state, for the onboarding screen (Defect 1: made
/// queryable instead of only ever `eprintln!`'d where a packaged .app can't show it).
pub fn hotkey_status(state: &AppState) -> HotkeyReport {
    state.hotkeys.lock().map(|g| g.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_report_all_registered_has_no_error() {
        let report = build_hotkey_report(Ok(()), Ok(()), Ok(()));
        assert!(report.toggle);
        assert!(report.browse);
        assert!(report.settings);
        assert!(report.error.is_none());
    }

    #[test]
    fn hotkey_report_records_only_the_first_failure() {
        // toggle (Cmd+Shift+K) is the important one users actually feel missing;
        // it fails here, and browse also fails, but only the FIRST failure's
        // message should surface (Defect 1: one clear cause, not three).
        let report = build_hotkey_report(
            Err("already registered by another application".to_string()),
            Err("boom".to_string()),
            Ok(()),
        );
        assert!(!report.toggle);
        assert!(!report.browse);
        assert!(report.settings);
        assert_eq!(
            report.error.as_deref(),
            Some("Cmd+Shift+K: already registered by another application")
        );
    }

    #[test]
    fn hotkey_report_a_later_failure_is_reported_when_earlier_ones_succeed() {
        let report = build_hotkey_report(Ok(()), Ok(()), Err("nope".to_string()));
        assert!(report.toggle);
        assert!(report.browse);
        assert!(!report.settings);
        assert_eq!(report.error.as_deref(), Some("Cmd+Shift+,: nope"));
    }
}
