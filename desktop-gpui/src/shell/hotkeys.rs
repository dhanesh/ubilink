//! The three global hotkeys (R4): ⌘⇧K capture, ⌘⇧L browser, ⌘⇧, settings — the same
//! bindings as the deleted Tauri shell had (its `hotkey_shortcuts`).
//!
//! TN4: global-hotkey 0.8 uses Carbon `RegisterEventHotKey` with a handler on the
//! application event target, so the manager must be created on the main thread once
//! NSApp's loop exists — i.e. inside GPUI's `Application::run` closure — and kept alive
//! for the app's lifetime (dropping it unregisters everything). Its events land in a
//! crossbeam channel that `shell::pump` drains from a GPUI foreground task.
//!
//! A failed registration (another app already owns the chord) is recorded per hotkey in
//! `ShellState` and logged, never a panic: the Tauri shell learned that a dead hotkey
//! has to be visible (ubilink-app `HotkeyReport`, Defect 1).

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::GlobalHotKeyManager;

use super::Route;

/// Env switch for automation instances (Task 6/8): don't grab the user's hotkeys.
pub const SKIP_ENV: &str = "UBILINK_SKIP_HOTKEYS";

/// `(display name, hotkey, route it opens)` for each binding.
pub fn bindings() -> [(&'static str, HotKey, Route); 3] {
    let mods = Some(Modifiers::SUPER | Modifiers::SHIFT);
    [
        ("Cmd+Shift+K", HotKey::new(mods, Code::KeyK), Route::Capture),
        ("Cmd+Shift+L", HotKey::new(mods, Code::KeyL), Route::Browser),
        (
            "Cmd+Shift+,",
            HotKey::new(mods, Code::Comma),
            Route::Settings,
        ),
    ]
}

/// What registration produced: the live manager (None if it could not be created or
/// registration was skipped), the hotkey-id → route map for the event pump, and the
/// report folded into `ShellState`.
pub struct Registration {
    pub manager: Option<GlobalHotKeyManager>,
    pub routes: Vec<(u32, Route)>,
    pub report: Report,
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub registered: Vec<String>,
    pub skipped: bool,
    pub error: Option<String>,
}

/// Pure: fold per-hotkey results into a `Report`. Unlike the Tauri report (first error
/// only) every failure is kept, joined by "; " — the probe and the user both need to
/// know which chords are dead.
pub fn fold(results: &[(&str, Result<(), String>)]) -> Report {
    let registered = results
        .iter()
        .filter(|(_, r)| r.is_ok())
        .map(|(n, _)| (*n).to_string())
        .collect();
    let errors: Vec<String> = results
        .iter()
        .filter_map(|(n, r)| r.as_ref().err().map(|e| format!("{n}: {e}")))
        .collect();
    Report {
        registered,
        skipped: false,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    }
}

/// Whether the env asks to skip registration ("1" or "true").
pub fn skip_requested(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true"))
}

/// Create the manager and register every binding. Must run on the main thread inside
/// the GPUI run closure (TN4).
pub fn register() -> Registration {
    let routes = bindings().iter().map(|(_, hk, r)| (hk.id(), *r)).collect();
    if skip_requested(std::env::var(SKIP_ENV).ok().as_deref()) {
        eprintln!("[ubilink] {SKIP_ENV}=1: global hotkeys not registered");
        return Registration {
            manager: None,
            routes,
            report: Report {
                skipped: true,
                ..Report::default()
            },
        };
    }
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            let msg = format!("hotkey manager unavailable: {e}");
            eprintln!("[ubilink] {msg}");
            return Registration {
                manager: None,
                routes,
                report: Report {
                    error: Some(msg),
                    ..Report::default()
                },
            };
        }
    };
    let results: Vec<(&str, Result<(), String>)> = bindings()
        .into_iter()
        .map(|(name, hk, _)| (name, manager.register(hk).map_err(|e| e.to_string())))
        .collect();
    let report = fold(&results);
    if let Some(err) = &report.error {
        eprintln!("[ubilink] hotkey registration issue: {err}");
    }
    Registration {
        manager: Some(manager),
        routes,
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_registered_has_no_error() {
        let r = fold(&[("A", Ok(())), ("B", Ok(())), ("C", Ok(()))]);
        assert_eq!(r.registered, vec!["A", "B", "C"]);
        assert_eq!(r.error, None);
    }

    #[test]
    fn every_failure_is_reported_by_name() {
        let r = fold(&[
            ("A", Err("taken".into())),
            ("B", Ok(())),
            ("C", Err("taken".into())),
        ]);
        assert_eq!(r.registered, vec!["B"]);
        assert_eq!(r.error.as_deref(), Some("A: taken; C: taken"));
    }

    #[test]
    fn skip_env_values() {
        assert!(skip_requested(Some("1")));
        assert!(skip_requested(Some("true")));
        assert!(!skip_requested(Some("0")));
        assert!(!skip_requested(None));
    }

    #[test]
    fn three_distinct_bindings_route_to_capture_browser_settings() {
        let b = bindings();
        let ids: std::collections::HashSet<u32> = b.iter().map(|(_, hk, _)| hk.id()).collect();
        assert_eq!(ids.len(), 3);
        let routes: Vec<Route> = b.iter().map(|(_, _, r)| *r).collect();
        assert_eq!(
            routes,
            vec![Route::Capture, Route::Browser, Route::Settings]
        );
    }
}
