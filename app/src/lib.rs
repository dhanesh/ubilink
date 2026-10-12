//! Satisfies: R3 (GPUI Phase 0 plan, task 3). The command-layer logic the app's
//! views call. It was the logic behind every `#[tauri::command]` in the Tauri shell
//! (`desktop/src/main.rs`, deleted in D24; last present at `ee80e70`), lifted out so
//! the GPUI app (`desktop-gpui/`) called the SAME functions instead of re-deriving
//! Tauri's behaviour — per D8/D11 the GPUI port was a UI port, not a logic port.
//!
//! Everything here is plain Rust over `ubilink_core`: no GUI types, just an explicit
//! `&AppState` (or the narrower inputs a function actually needs, e.g.
//! `export_graph`'s save directory). The app's views supply those inputs from their
//! own runtime and read the same return shapes the Tauri web panel did (see
//! task-3-report.md).
//!
//! What never moved here (and why) is also in that report: window geometry, app
//! restart and the global-shortcut registration path — all of them need a live GUI
//! runtime, which this crate must never depend on (see `deps-clean` below). In the
//! GPUI app they live in `desktop-gpui/src/shell/`.

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
pub use capture::{
    capture_app, capture_from, capture_now, detect_frontmost, source_app_name, Captured,
};
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
