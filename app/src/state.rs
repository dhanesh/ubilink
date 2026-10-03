//! The shared state every command reads/writes, moved verbatim out of
//! `desktop/src/main.rs`'s `AppState` (D11). None of its fields are Tauri types —
//! `Mutex`/`HashSet`/`PathBuf` are std, `PanelBackend`/`AdapterRegistry` are
//! ubilink-core — so the shell constructs this struct directly and hands `&AppState`
//! to every function below instead of `tauri::State`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ubilink_core::{AdapterRegistry, PanelBackend};

use crate::HotkeyReport;

/// App state: the link-graph-backed panel backend + the declarative adapter registry.
pub struct AppState {
    pub backend: Mutex<PanelBackend>,
    pub adapters: AdapterRegistry,
    /// App names (lowercased) of non-built-in adapters the user has turned OFF.
    /// Built-in adapters can't be disabled, so they never appear here.
    pub disabled: Mutex<HashSet<String>>,
    /// Where plugins-state.json lives (so toggles persist across launches).
    pub data_dir: PathBuf,
    /// Latest hotkey registration outcome (Defect 1/2): set at startup, and again
    /// by `retry_hotkeys`; read by `hotkey_status`.
    pub hotkeys: Mutex<HotkeyReport>,
}

/// Read the persisted set of disabled plugin app-names (lowercased). Missing/garbled
/// file → empty (everything enabled). Tiny std-only JSON via serde_json.
pub fn load_disabled(data_dir: &Path) -> HashSet<String> {
    let path = data_dir.join("plugins-state.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Default::default();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Default::default();
    };
    v.get("disabled")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// Persist the disabled set. Best-effort (a failed write just means the toggle
/// won't survive a restart — surfaced as the command's error).
pub fn save_disabled(data_dir: &Path, disabled: &HashSet<String>) -> Result<(), String> {
    let mut list: Vec<&String> = disabled.iter().collect();
    list.sort();
    let body = serde_json::json!({ "disabled": list });
    std::fs::write(data_dir.join("plugins-state.json"), body.to_string()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        // Unique per test (name + pid) so parallel test threads don't clobber each other.
        let d =
            std::env::temp_dir().join(format!("ubilink-app-plugins-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn disabled_set_round_trips() {
        let dir = tmp_dir("roundtrip");
        let mut set = HashSet::new();
        set.insert("bear".to_string());
        set.insert("things3".to_string());
        save_disabled(&dir, &set).unwrap();
        let back = load_disabled(&dir);
        assert_eq!(back, set);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_empty() {
        // A never-written data dir → nothing disabled (everything enabled).
        let dir = std::env::temp_dir().join("ubilink-app-plugins-nonexistent-xyz");
        std::fs::remove_dir_all(&dir).ok();
        assert!(load_disabled(&dir).is_empty());
    }

    #[test]
    fn garbled_file_is_empty() {
        // A corrupt plugins-state.json must not panic — degrade to "all enabled".
        let dir = tmp_dir("garbled");
        std::fs::write(dir.join("plugins-state.json"), b"{ not json").unwrap();
        assert!(load_disabled(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
