//! `capture_current`'s logic (D11/D12): detect the frontmost app, then capture its
//! resource. Moved verbatim from the deleted Tauri shell (`desktop/src/main.rs`,
//! D24) — no GUI type appears anywhere in this path, so the GPUI capture view calls
//! `capture_now` directly.

use std::time::{SystemTime, UNIX_EPOCH};

use ubilink_core::{
    CommandRunner, ConfiguredAdapter, OsaScriptRunner, PanelBackend, PanelView,
    FRONTMOST_APP_SCRIPT,
};

use crate::AppState;

/// Detect the frontmost application RIGHT NOW (osascript). Split out of
/// `capture_now` (Defect 3 — the panel used to be shown before it was focused)
/// so the ⌘⇧K handler can detect first, focus the panel immediately, and only
/// then do the slower per-app capture work — WITHOUT moving `set_focus()` ahead
/// of detection itself, which would risk the panel becoming key window before we
/// know which app to capture from and breaking the product's core feature.
pub fn detect_frontmost() -> Result<String, String> {
    OsaScriptRunner
        .run("osascript", &["-e", FRONTMOST_APP_SCRIPT])
        .map_err(|e| format!("Couldn't tell which app is active: {e}"))
}

/// Capture the resource of `frontmost`: look up its declarative adapter
/// (adapters_config), capture+validate+surface-backlinks. Everything `capture_now`
/// used to do once it had a frontmost-app name in hand. U3 explains an unsupported
/// app instead of failing silently.
pub fn capture_from(state: &AppState, frontmost: &str) -> Result<PanelView, String> {
    let spec = state.adapters.for_app(frontmost).ok_or_else(|| {
        format!(
            "Linking from \u{201c}{}\u{201d} isn't supported yet.",
            frontmost.trim()
        )
    })?;
    // Honor the plugin manager: a non-built-in adapter the user turned OFF behaves as
    // unsupported (built-ins can't be disabled, so they always pass).
    if !spec.builtin {
        let disabled = state
            .disabled
            .lock()
            .map_err(|_| "internal state was poisoned".to_string())?;
        if disabled.contains(&spec.app.to_ascii_lowercase()) {
            return Err(format!(
                "Linking from \u{201c}{}\u{201d} is turned off in Settings.",
                spec.app
            ));
        }
    }
    let adapter = ConfiguredAdapter::new(spec.clone(), OsaScriptRunner);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // Takes the backend lock only around its fast reads/writes, never across the
    // (possibly stalled) osascript call, so other commands stay responsive.
    PanelBackend::capture_current(&state.backend, &adapter, now)
}

/// A capture and the app it came from (rc.6): the capture view shows the source app's
/// name, with its icon, beside the captured link.
#[derive(Debug, Clone)]
pub struct Captured {
    /// The frontmost app's name as macOS reported it (`detect_frontmost`), trimmed.
    pub app: String,
    /// The captured resource and its backlinks.
    pub panel: PanelView,
}

/// The name a capture shows for `frontmost` (osascript's answer ends in a newline).
pub fn source_app_name(frontmost: &str) -> String {
    frontmost.trim().to_string()
}

/// [`capture_from`], keeping the source app's name with the result.
pub fn capture_app(state: &AppState, frontmost: &str) -> Result<Captured, String> {
    capture_from(state, frontmost).map(|panel| Captured {
        app: source_app_name(frontmost),
        panel,
    })
}

/// Capture the resource of whatever app is frontmost right now: detect it, then
/// capture from it, keeping the app's name. Kept as the composition of
/// `detect_frontmost` + `capture_app` so callers can also run the two halves
/// separately (the Tauri shell's ⌘⇧K handler did, to focus the panel in between).
pub fn capture_now(state: &AppState) -> Result<Captured, String> {
    let frontmost = detect_frontmost()?;
    capture_app(state, &frontmost)
}

#[cfg(test)]
mod tests {
    use super::source_app_name;

    /// osascript's answer carries a trailing newline; the view shows the bare name.
    #[test]
    fn the_source_app_name_is_trimmed() {
        assert_eq!(source_app_name("Safari\n"), "Safari");
        assert_eq!(source_app_name("  Google Chrome \n"), "Google Chrome");
        assert_eq!(source_app_name("Bear"), "Bear");
    }
}
