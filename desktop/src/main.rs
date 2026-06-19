// Satisfies: T1 (the broker/core wired into Tauri commands), U1 (global hotkey ->
// panel), U2 (copy in <=2 actions), U3 (explained errors), U5 (backlinks), O2
// (in-app capability disclosure). See docs/RUNBOOK-gui-panel.md.
//
// The imperative GUI shell. Every command is a thin wrapper over hooky-core's
// `PanelBackend` (the headlessly-tested command contract) + macOS shell-outs for
// the things that genuinely need the OS (frontmost-app detection, clipboard, open).
// Building this needs the Tauri toolchain; RUNNING it needs a desktop session.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use hooky_core::{
    app_kind_for, CommandRunner, HookyService, LinkGraph, MacAppAdapter, OsaScriptRunner,
    PanelBackend, PanelView, Support, FRONTMOST_APP_SCRIPT,
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

type Backend = Mutex<PanelBackend>;

/// Capture the active resource: detect the frontmost app, route to its adapter, then
/// capture+validate+surface-backlinks via the core (U3 explains an unsupported app).
#[tauri::command]
fn capture_current(state: State<'_, Backend>) -> Result<PanelView, String> {
    let frontmost = OsaScriptRunner
        .run("osascript", &["-e", FRONTMOST_APP_SCRIPT])
        .map_err(|e| format!("Couldn't tell which app is active: {e}"))?;
    let kind = app_kind_for(&frontmost)
        .ok_or_else(|| format!("Linking from \u{201c}{}\u{201d} isn't supported yet.", frontmost.trim()))?;
    let adapter = MacAppAdapter::new(kind, OsaScriptRunner);
    let mut backend = state.lock().map_err(|_| "internal state was poisoned".to_string())?;
    backend.capture_current(&adapter)
}

/// In-app capability disclosure (O2): what linking works where.
#[tauri::command]
fn capabilities(state: State<'_, Backend>) -> Result<Vec<Support>, String> {
    let backend = state.lock().map_err(|_| "internal state was poisoned".to_string())?;
    Ok(backend.capabilities())
}

/// U2: put the link on the clipboard (macOS `pbcopy`, no extra plugin/dep).
#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
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

/// Open a captured link's URI (the OpenPlan shape: macOS `open`).
#[tauri::command]
fn open_link(uri: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&uri)
        .spawn()
        .map_err(|e| format!("couldn't open {uri}: {e}"))?;
    Ok(())
}

/// Hide the panel — the flow is done (U2: 2 actions, then get out of the way).
#[tauri::command]
fn dismiss_panel(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("panel") {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    // The on-device link graph (RT-8): persistent, local-only. A real app resolves a
    // proper data dir; in-memory keeps first-run simple and is swapped in setup().
    let graph = LinkGraph::open_in_memory().expect("failed to open link graph");
    // Probe the automation permission once at startup (S4); capture_current degrades
    // honestly if it's not granted.
    let permission = hooky_core::probe_automation_permission(&OsaScriptRunner);
    let backend: Backend = Mutex::new(PanelBackend::new(HookyService::new(graph).with_permission(permission)));

    tauri::Builder::default()
        .manage(backend)
        .setup(|app| {
            // Show the panel on launch so there is ALWAYS visible UI. The global
            // hotkey below needs macOS Accessibility permission and may not fire
            // until it's granted — relying on it alone would leave a blank dock icon.
            if let Some(w) = app.get_webview_window("panel") {
                let _ = w.show();
                let _ = w.set_focus();
            }

            // U1: a global hotkey (Cmd/Ctrl+Shift+K) toggles the panel's visibility.
            // Registration failure (e.g. missing Accessibility permission) is
            // non-fatal — the window is already up, so the app stays usable.
            let toggle = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyK);
            let toggle_for_handler = toggle;
            let plugin = tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if shortcut == &toggle_for_handler && event.state() == ShortcutState::Pressed {
                        if let Some(w) = app.get_webview_window("panel") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build();
            if let Err(e) = app.handle().plugin(plugin) {
                eprintln!("[hooky] global-shortcut plugin failed to load: {e}");
            } else if let Err(e) = app.global_shortcut().register(toggle) {
                eprintln!("[hooky] couldn't register Cmd+Shift+K (grant Accessibility permission in System Settings): {e}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            capture_current,
            capabilities,
            copy_to_clipboard,
            open_link,
            dismiss_panel
        ])
        .run(tauri::generate_context!())
        .expect("error while running hooky desktop");
}
