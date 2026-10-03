//! `open_link`: re-validate a stored URI with the capture policy and launch it.
//! Moved verbatim, including `SpawnRunner` — a `CommandRunner` impl, not a Tauri
//! type (`ubilink_core::CommandRunner` is the I/O seam core already defines).

use ubilink_core::CommandRunner;

use crate::AppState;

/// Runs a program without waiting for it (`open` hands off to LaunchServices, so
/// there is nothing to wait for on the command thread). Used by `open_link`.
pub struct SpawnRunner;

impl CommandRunner for SpawnRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        std::process::Command::new(program)
            .args(args)
            .spawn()
            .map(|_| String::new())
            .map_err(|e| format!("couldn't run {program}: {e}"))
    }
}

/// Open a captured link's URI. The core re-validates it with the capture policy (a
/// stored row may predate validation) and builds the command so the URI can't be
/// read as an option or reach a shell (macOS: `open -- <uri>`).
pub fn open_link(state: &AppState, uri: &str) -> Result<(), String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.open_link(&SpawnRunner, uri)
}
