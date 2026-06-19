// Satisfies: T1 (broker/core wired into Tauri commands), U1 (global hotkey -> panel,
// capturing the app you were IN, not the panel itself), U2 (copy in <=2 actions), U3
// (explained errors + permission onboarding), U5 (backlinks), O2 (capability
// disclosure), S4/RT-2 (permission detection + guided granting).
// See docs/RUNBOOK-gui-panel.md.
//
// The imperative GUI shell. Logic lives in hooky-core (headlessly tested); this wires
// it to Tauri + the macOS bits that genuinely need the OS (frontmost-app detection,
// clipboard, opening URLs, permission checks). Building needs the Tauri toolchain;
// running needs a desktop session.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use hooky_core::{
    app_kind_for, CommandRunner, HookyService, LinkGraph, MacAppAdapter, OsaScriptRunner,
    PanelBackend, PanelView, PermissionState, Support, FRONTMOST_APP_SCRIPT,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

type Backend = Mutex<PanelBackend>;

/// Capture the resource of whatever app is frontmost RIGHT NOW: detect it, route to
/// its adapter, capture+validate+surface-backlinks. Called from the hotkey handler
/// (before the panel steals focus) and the `capture_current` command. U3 explains an
/// unsupported app instead of failing silently.
fn capture_now(backend: &Backend) -> Result<PanelView, String> {
    let frontmost = OsaScriptRunner
        .run("osascript", &["-e", FRONTMOST_APP_SCRIPT])
        .map_err(|e| format!("Couldn't tell which app is active: {e}"))?;
    let kind = app_kind_for(&frontmost)
        .ok_or_else(|| format!("Linking from \u{201c}{}\u{201d} isn't supported yet.", frontmost.trim()))?;
    let adapter = MacAppAdapter::new(kind, OsaScriptRunner);
    let mut b = backend.lock().map_err(|_| "internal state was poisoned".to_string())?;
    b.capture_current(&adapter)
}

#[tauri::command]
fn capture_current(state: State<'_, Backend>) -> Result<PanelView, String> {
    capture_now(state.inner())
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

/// The two macOS permissions hooky needs, and their current state — so the
/// onboarding screen can tell the user exactly what's left to do (S4/RT-2/U3).
#[derive(serde::Serialize, Clone)]
struct PermissionReport {
    /// Accessibility (AXIsProcessTrusted) — required for the global hotkey to fire.
    accessibility: bool,
    /// Automation (System Events, via osascript): "granted" | "denied" | "not_determined".
    automation: &'static str,
}

/// Whether this process is trusted for the macOS Accessibility API (the global
/// hotkey needs it). Direct FFI — no crate, keeping the dep tree honest.
#[cfg(target_os = "macos")]
fn accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}
#[cfg(not(target_os = "macos"))]
fn accessibility_trusted() -> bool {
    false
}

/// Report current permission state (called by the onboarding screen + its Re-check).
#[tauri::command]
fn permission_status() -> PermissionReport {
    let automation = match hooky_core::probe_automation_permission(&OsaScriptRunner) {
        PermissionState::Granted => "granted",
        PermissionState::Denied => "denied",
        PermissionState::NotDetermined => "not_determined",
    };
    PermissionReport { accessibility: accessibility_trusted(), automation }
}

/// Open the relevant System Settings privacy pane so the user can grant a permission.
#[tauri::command]
fn open_settings(pane: String) -> Result<(), String> {
    let url = match pane.as_str() {
        "accessibility" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "automation" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
        other => return Err(format!("unknown settings pane: {other}")),
    };
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|e| format!("couldn't open System Settings: {e}"))?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // The on-device link graph (RT-8): a persistent SQLite file in the OS
            // app-data dir, so hooks survive restarts. Resolved here because the
            // path API needs the running app. Local-only — never leaves the device.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("hooky-graph.sqlite");
            let graph = LinkGraph::open(&db_path.to_string_lossy())
                .map_err(|e| format!("couldn't open link graph at {}: {e}", db_path.display()))?;
            eprintln!("[hooky] link graph: {}", db_path.display());

            // Probe automation permission at startup (S4); capture degrades honestly
            // if it's not granted, and the onboarding screen re-checks on demand.
            let permission = hooky_core::probe_automation_permission(&OsaScriptRunner);
            app.manage::<Backend>(Mutex::new(PanelBackend::new(
                HookyService::new(graph).with_permission(permission),
            )));

            // Show the panel on launch so there is ALWAYS visible UI. On load the
            // webview checks permissions and shows onboarding if anything's missing.
            if let Some(w) = app.get_webview_window("panel") {
                let _ = w.show();
                let _ = w.set_focus();
            }

            // U1: the global hotkey CAPTURES the app that is frontmost right now —
            // before we show the panel and steal focus — then pushes the result to
            // the webview and reveals the panel. So it links the app you were IN, and
            // re-captures on every press. (Esc dismisses; see panel.js.)
            let toggle = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyK);
            let toggle_for_handler = toggle;
            let plugin = tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if shortcut == &toggle_for_handler && event.state() == ShortcutState::Pressed {
                        if let Some(w) = app.get_webview_window("panel") {
                            let state = app.state::<Backend>();
                            match capture_now(state.inner()) {
                                Ok(view) => {
                                    let _ = w.emit("capture-ok", view);
                                }
                                Err(msg) => {
                                    let _ = w.emit("capture-err", msg);
                                }
                            }
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build();
            if let Err(e) = app.handle().plugin(plugin) {
                eprintln!("[hooky] global-shortcut plugin failed to load: {e}");
            } else if let Err(e) = app.global_shortcut().register(toggle) {
                eprintln!("[hooky] couldn't register Cmd+Shift+K (grant Accessibility permission): {e}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            capture_current,
            capabilities,
            copy_to_clipboard,
            open_link,
            dismiss_panel,
            permission_status,
            open_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running hooky desktop");
}
