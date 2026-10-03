//! Satisfies: R3 (GPUI Phase 0 plan, task 3). The command-layer logic behind every
//! `#[tauri::command]` in `desktop/src/main.rs`, lifted out so a second shell
//! (GPUI, `desktop-gpui/`) can call the SAME functions instead of re-deriving
//! Tauri's behaviour — per D8/D11 the GPUI port is a UI port, not a logic port.
//!
//! Everything here is plain Rust over `ubilink_core`: no `tauri::State`/`AppHandle`,
//! just an explicit `&AppState` (or the narrower inputs a function actually needs,
//! e.g. `export_graph`'s save directory). A shell wraps each function in a thin
//! `#[tauri::command]`/GPUI-action adapter that supplies those inputs from its own
//! runtime and keeps the JS-facing command name, argument names and return JSON
//! shape unchanged (dist/panel.js depends on them — see task-3-report.md).
//!
//! What stayed in `desktop/src/main.rs` instead (and why) is also in that report:
//! window geometry (`resize_panel`, `dismiss_panel`), `restart_app`
//! (`AppHandle::restart`), and the global-shortcut registration path
//! (`retry_hotkeys` + its `register_hotkeys`/`hotkey_shortcuts` helpers) — all of
//! them need a live `tauri::AppHandle` or a `tauri_plugin_global_shortcut::Shortcut`,
//! which this crate must never depend on (see `deps-clean` below).

mod browse;
mod capture;
mod clipboard;
mod hotkeys;
mod open_link;
mod permissions;
mod plugins;
mod settings;
mod state;

pub use browse::{link_resources, links_of, list_links};
pub use capture::{capture_from, capture_now, detect_frontmost};
pub use clipboard::copy_to_clipboard;
pub use hotkeys::{build_hotkey_report, hotkey_status, HotkeyReport};
pub use open_link::{open_link, open_link_with, SpawnRunner};
pub use permissions::{
    accessibility_trusted, open_settings, open_settings_with, permission_status,
    prompt_accessibility, request_accessibility, settings_pane_url, PermissionReport,
};
pub use plugins::{list_plugins, set_plugin_enabled, PluginView};
pub use settings::{capabilities, export_graph, graph_stats, import_graph, reset_graph};
pub use state::{load_disabled, save_disabled, AppState};
