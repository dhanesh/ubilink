//! The link-browser's three commands (list, drill-in, link). Moved verbatim from
//! `desktop/src/main.rs`; `link_resources` keeps supplying its own clock reading
//! (ubilink-core has no clock) since `SystemTime` is std, not Tauri — so this stays
//! a genuine logic move rather than leaving the timestamp in the shell.

use std::time::{SystemTime, UNIX_EPOCH};

use ubilink_core::BrowseEntry;

use crate::AppState;

/// The link-browser top level (Cmd+Shift+L): every captured resource, title+uri.
pub fn list_links(state: &AppState) -> Result<Vec<BrowseEntry>, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.list_links()
}

/// Drill-in (→): everything linked to `uri`.
pub fn links_of(state: &AppState, uri: &str) -> Result<Vec<BrowseEntry>, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.links_of(uri)
}

/// Link two captured resources (browser "L"). The core has no clock, so the
/// caller supplies the timestamp.
pub fn link_resources(state: &AppState, a_uri: &str, b_uri: &str) -> Result<(), String> {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64);
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.link_resources(a_uri, b_uri, created_at)
}
