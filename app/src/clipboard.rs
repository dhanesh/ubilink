//! U2: put the link on the clipboard. Moved verbatim — `pbcopy` is a plain
//! process spawn, no Tauri involved.

/// U2: put the link on the clipboard (macOS `pbcopy`, no extra plugin/dep).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("clipboard unavailable: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or("clipboard pipe unavailable")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}
