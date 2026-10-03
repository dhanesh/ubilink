//! The Settings panel's commands: graph stats/reset/export/import and the O2
//! capability disclosure. Moved verbatim from `desktop/src/main.rs`, except
//! `export_graph`: the original command resolved its save directory from
//! `AppHandle::path()` (genuinely Tauri — stays in the shell) and then did
//! everything else (read the graph, stamp a filename, write the file) in plain
//! Rust. That everything-else part is what moved here, taking the resolved `dir`
//! as a plain `&Path` so the shell's wrapper supplies it without exposing
//! `AppHandle` to this crate.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ubilink_core::{GraphStats, Support};

use crate::AppState;

/// Settings: resource + link counts.
pub fn graph_stats(state: &AppState) -> Result<GraphStats, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.graph_stats()
}

/// Settings: wipe the graph (destructive — the webview confirms first). Returns
/// the number of resources removed.
pub fn reset_graph(state: &AppState) -> Result<i64, String> {
    let mut backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.reset_graph()
}

/// Settings: export the graph as JSON to `dir` (the shell resolves `dir`, e.g.
/// `app.path().download_dir()`, since that needs `AppHandle`), returning the
/// written path. The shell is left to reveal it in Finder — that's an OS-shell
/// concern, not graph logic.
pub fn export_graph(state: &AppState, dir: &Path) -> Result<String, String> {
    let json = {
        let backend = state
            .backend
            .lock()
            .map_err(|_| "internal state was poisoned".to_string())?;
        backend.export_graph()?
    };
    // No clock in core; stamp the filename from the OS time here.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("ubilink-export-{stamp}.json"));
    std::fs::write(&path, json).map_err(|e| format!("couldn't write export: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Settings: import a graph-export JSON (read by the webview's file input).
/// Merges into the current graph; returns a human summary.
pub fn import_graph(state: &AppState, json: &str) -> Result<String, String> {
    let mut backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.import_graph(json)
}

/// In-app capability disclosure (O2): what linking works where.
pub fn capabilities(state: &AppState) -> Result<Vec<Support>, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    Ok(backend.capabilities())
}
