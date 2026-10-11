//! The Settings panel's commands: graph stats/reset/export/import and the O2
//! capability disclosure. Moved verbatim from `desktop/src/main.rs`, except
//! `export_graph`: the original command resolved its save directory from
//! `AppHandle::path()` (genuinely Tauri — stays in the shell) and then did
//! everything else (read the graph, stamp a filename, write the file) in plain
//! Rust. That everything-else part is what moved here, taking the resolved `dir`
//! as a plain `&Path` so the shell's wrapper supplies it without exposing
//! `AppHandle` to this crate.

use std::path::{Path, PathBuf};
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

/// Settings: wipe the graph (destructive — the Settings view confirms first). Returns
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
    let path = write_new_file(
        dir,
        &format!("ubilink-export-{stamp}"),
        "json",
        json.as_bytes(),
    )
    .map_err(|e| format!("couldn't write export: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Write `contents` to a NEW file `<stem>.<ext>` in `dir`, or `<stem>-1.<ext>`,
/// `<stem>-2.<ext>`, ... when that name is taken. Never overwrites a file and never
/// writes through a symlink: each name is created with `create_new` (O_EXCL), which
/// fails on anything already there, a symlink included. Two exports in the same second
/// get two files.
pub fn write_new_file(
    dir: &Path,
    stem: &str,
    ext: &str,
    contents: &[u8],
) -> std::io::Result<PathBuf> {
    use std::io::Write;
    for n in 0..1000u32 {
        let name = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(contents)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{stem}.{ext} and 999 numbered names are all taken"),
    ))
}

/// Settings: import a graph-export JSON (read from a fixture or an open dialog's file).
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

#[cfg(test)]
mod write_new_file_tests {
    use super::write_new_file;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ubilink-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_taken_name_gets_a_numbered_one_and_nothing_is_overwritten() {
        let d = temp_dir("clash");
        let a = write_new_file(&d, "ubilink-export-7", "json", b"first").unwrap();
        let b = write_new_file(&d, "ubilink-export-7", "json", b"second").unwrap();
        let c = write_new_file(&d, "ubilink-export-7", "json", b"third").unwrap();
        assert_eq!(a, d.join("ubilink-export-7.json"));
        assert_eq!(b, d.join("ubilink-export-7-1.json"));
        assert_eq!(c, d.join("ubilink-export-7-2.json"));
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        assert_eq!(std::fs::read(&b).unwrap(), b"second");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_at_the_name_is_never_written_through() {
        let d = temp_dir("symlink");
        let target = d.join("target.txt");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, d.join("x.json")).unwrap();
        let p = write_new_file(&d, "x", "json", b"export").unwrap();
        assert_eq!(p, d.join("x-1.json"));
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        std::fs::remove_dir_all(&d).ok();
    }
}
