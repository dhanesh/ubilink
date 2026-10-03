//! Settings › App adapters: the plugin-manager rows and the enable/disable toggle.
//! Moved verbatim from `desktop/src/main.rs`, including `PluginView` (a
//! logic-level view type — plain data mapped from `AdapterRegistry`, nothing
//! Tauri-specific about it).

use serde::Serialize;

use crate::AppState;

/// One row in the plugin manager: a supported app, whether it's a built-in
/// default-macOS adapter, whether it's a custom adapter from the user's
/// `adapters.json` (trusted local config running with the user's permissions,
/// not a sandboxed plugin — D14), and whether it's enabled.
#[derive(Serialize, Clone)]
pub struct PluginView {
    pub app: String,
    pub builtin: bool,
    pub user_defined: bool,
    pub enabled: bool,
    pub scheme: Option<String>,
}

/// Settings › App adapters: every adapter as a toggle row (built-in / shipped /
/// custom, enabled/disabled). Sorted case-insensitively by app name for a stable list.
pub fn list_plugins(state: &AppState) -> Result<Vec<PluginView>, String> {
    let disabled = state
        .disabled
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    let mut rows: Vec<PluginView> = state
        .adapters
        .specs()
        .iter()
        .map(|s| PluginView {
            app: s.app.clone(),
            builtin: s.builtin,
            user_defined: s.user_defined,
            // Built-ins are always on; any other adapter is on unless the user disabled it.
            enabled: s.builtin || !disabled.contains(&s.app.to_ascii_lowercase()),
            scheme: s.schemes.first().cloned(),
        })
        .collect();
    rows.sort_by_key(|p| p.app.to_ascii_lowercase());
    Ok(rows)
}

/// Settings › App adapters: toggle a non-built-in adapter on/off and persist it.
/// Built-in adapters (core macOS apps ubilink ships with) can't be disabled; a
/// user `adapters.json` entry is never built-in (D14), so it can always be disabled.
pub fn set_plugin_enabled(state: &AppState, app: &str, enabled: bool) -> Result<(), String> {
    let is_builtin = state
        .adapters
        .specs()
        .iter()
        .any(|s| s.app.eq_ignore_ascii_case(app) && s.builtin);
    if is_builtin && !enabled {
        return Err(format!(
            "{app} is a built-in adapter and can't be disabled."
        ));
    }
    let key = app.to_ascii_lowercase();
    let mut disabled = state
        .disabled
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    if enabled {
        disabled.remove(&key);
    } else {
        disabled.insert(key);
    }
    crate::save_disabled(&state.data_dir, &disabled)
}
