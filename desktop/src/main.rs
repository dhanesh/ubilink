// Satisfies: T1 (broker/core wired into Tauri commands), U1 (global hotkey -> panel,
// capturing the app you were IN, not the panel itself), U2 (copy in <=2 actions), U3
// (explained errors + permission onboarding), U5 (backlinks), O2 (capability
// disclosure), S4/RT-2 (permission detection + guided granting).
// See docs/RUNBOOK-gui-panel.md.
//
// The imperative GUI shell. Logic lives in ubilink-core (headlessly tested); this wires
// it to Tauri + the macOS bits that genuinely need the OS (frontmost-app detection,
// clipboard, opening URLs, permission checks). Building needs the Tauri toolchain;
// running needs a desktop session.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ubilink_core::{
    AdapterRegistry, CommandRunner, ConfiguredAdapter, UbilinkService, LinkGraph, OsaScriptRunner,
    PanelBackend, PanelView, PermissionState, Support, FRONTMOST_APP_SCRIPT,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// App state: the link-graph-backed panel backend + the declarative adapter registry.
struct AppState {
    backend: Mutex<PanelBackend>,
    adapters: AdapterRegistry,
}

/// Capture the resource of whatever app is frontmost RIGHT NOW: detect it, look up
/// its declarative adapter (adapters_config), capture+validate+surface-backlinks.
/// Called from the hotkey handler (before the panel steals focus) and the
/// `capture_current` command. U3 explains an unsupported app instead of failing.
fn capture_now(state: &AppState) -> Result<PanelView, String> {
    let frontmost = OsaScriptRunner
        .run("osascript", &["-e", FRONTMOST_APP_SCRIPT])
        .map_err(|e| format!("Couldn't tell which app is active: {e}"))?;
    let spec = state
        .adapters
        .for_app(&frontmost)
        .ok_or_else(|| format!("Linking from \u{201c}{}\u{201d} isn't supported yet.", frontmost.trim()))?;
    let adapter = ConfiguredAdapter::new(spec.clone(), OsaScriptRunner);
    let mut b = state.backend.lock().map_err(|_| "internal state was poisoned".to_string())?;
    b.capture_current(&adapter)
}

#[tauri::command]
fn capture_current(state: State<'_, AppState>) -> Result<PanelView, String> {
    capture_now(state.inner())
}

/// In-app capability disclosure (O2): what linking works where.
#[tauri::command]
fn capabilities(state: State<'_, AppState>) -> Result<Vec<Support>, String> {
    let backend = state.backend.lock().map_err(|_| "internal state was poisoned".to_string())?;
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

/// The two macOS permissions ubilink needs, and their current state — so the
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

/// Trigger the macOS "grant Accessibility" system dialog
/// (`AXIsProcessTrustedWithOptions` with the prompt option) and return the current
/// trust state. Needed for UI-scripting app menus (e.g. Bear's "Copy Link"). Builds
/// the options CFDictionary by hand — dep-free CoreFoundation FFI.
#[cfg(target_os = "macos")]
fn prompt_accessibility() -> bool {
    use std::ffi::c_void;
    use std::ptr;
    type CFRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFRef;
        static kCFTypeDictionaryKeyCallBacks: [u8; 0];
        static kCFTypeDictionaryValueCallBacks: [u8; 0];
        fn CFDictionaryCreate(
            allocator: CFRef,
            keys: *const CFRef,
            values: *const CFRef,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFRef;
        fn CFRelease(cf: CFRef);
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: CFRef;
        fn AXIsProcessTrustedWithOptions(options: CFRef) -> u8;
    }
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::addr_of!(kCFTypeDictionaryKeyCallBacks) as *const c_void,
            ptr::addr_of!(kCFTypeDictionaryValueCallBacks) as *const c_void,
        );
        let trusted = AXIsProcessTrustedWithOptions(options) != 0;
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    }
}
#[cfg(not(target_os = "macos"))]
fn prompt_accessibility() -> bool {
    false
}

/// Show the macOS Accessibility prompt and report whether trust is now granted.
#[tauri::command]
fn request_accessibility() -> bool {
    prompt_accessibility()
}

/// Report current permission state (called by the onboarding screen + its Re-check).
#[tauri::command]
fn permission_status() -> PermissionReport {
    let automation = match ubilink_core::probe_automation_permission(&OsaScriptRunner) {
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
            // app-data dir, so links survive restarts. Resolved here because the
            // path API needs the running app. Local-only — never leaves the device.
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("ubilink-graph.sqlite");
            let graph = LinkGraph::open(&db_path.to_string_lossy())
                .map_err(|e| format!("couldn't open link graph at {}: {e}", db_path.display()))?;
            eprintln!("[ubilink] link graph: {}", db_path.display());

            // Probe automation permission at startup (S4); capture degrades honestly
            // if it's not granted, and the onboarding screen re-checks on demand.
            let permission = ubilink_core::probe_automation_permission(&OsaScriptRunner);

            // Declarative adapters (B3): built-in defaults, extended by an optional
            // user adapters.json in the data dir — add an app with NO recompile.
            let mut adapters = AdapterRegistry::defaults();
            let user_adapters = data_dir.join("adapters.json");
            if let Ok(json) = std::fs::read_to_string(&user_adapters) {
                match adapters.extend_from_json(&json) {
                    Ok(n) => eprintln!("[ubilink] loaded {n} user adapter(s) from {}", user_adapters.display()),
                    Err(e) => eprintln!("[ubilink] ignoring invalid {}: {e}", user_adapters.display()),
                }
            }
            eprintln!("[ubilink] {} app adapter(s) available", adapters.apps().len());

            // The bounded API (S5) must trust the schemes the adapters declare
            // (bear:, things:, …) — otherwise a captured bear:// link is rejected.
            let schemes = adapters.declared_schemes();
            app.manage(AppState {
                backend: Mutex::new(PanelBackend::new(
                    UbilinkService::new(graph).with_permission(permission).allow_schemes(schemes),
                )),
                adapters,
            });

            // U1 render-first: run as an Accessory app (overlay-style, like Spotlight/
            // Raycast). Showing the panel then does NOT make ubilink the frontmost
            // *application*, so the async capture below still sees the app you were in
            // — which lets us paint the panel BEFORE the slow osascript capture runs.
            // Trade-off: no Dock icon (summon with the hotkey; Esc dismisses).
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // With no Dock icon, the menu-bar tray is the always-reachable home for the
            // app: Show (summon the panel) + Quit (the affordance the Dock used to give).
            let show_i = MenuItem::with_id(app, "show", "Show ubilink", true, Some("Cmd+Shift+K"))?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit ubilink", true, Some("Cmd+Q"))?;
            let tray_menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            let mut tray = TrayIconBuilder::with_id("ubilink-tray")
                .tooltip("Ubilink — ⌘⇧K to capture a link")
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(w) = app.get_webview_window("panel") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    _ => {}
                });
            // A monochrome link glyph as a macOS TEMPLATE image: macOS recolors it
            // for the light/dark menu bar (black square -> adaptive). Falls back to the
            // app icon if decoding ever fails. Source SVG lives beside it for re-export.
            match tauri::image::Image::from_bytes(include_bytes!("../icons/ubilink-link.png")) {
                Ok(icon) => tray = tray.icon(icon).icon_as_template(true),
                Err(_) => {
                    if let Some(icon) = app.default_window_icon() {
                        tray = tray.icon(icon.clone());
                    }
                }
            }
            tray.build(app)?;

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
                            // U1 render-first: paint the spinner and reveal the panel NOW,
                            // before the slow (osascript) capture. The 'capturing' event
                            // drives the webview into its spinner state; its optional t0
                            // payload (epoch-ms, opt-in via UBILINK_PERF) lets panel.js measure
                            // Date.now()-t0 at first paint — i.e. the true hotkey->visible
                            // latency U1 governs (NOT hotkey->link-ready, which is bounded by
                            // osascript and can't go sub-200ms). Same machine = same wall clock.
                            let t0: Option<u64> = if std::env::var_os("UBILINK_PERF").is_some() {
                                SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
                            } else {
                                None
                            };
                            let _ = w.emit("capturing", t0);
                            let _ = w.show();

                            // Run the capture OFF the UI path so it never blocks first paint.
                            // As an Accessory app, show() above didn't steal the frontmost
                            // application, so capture_now still detects the app you were in.
                            // When osascript returns, push the result and focus the panel so
                            // Enter (U2) works.
                            let app = app.clone();
                            std::thread::spawn(move || {
                                let result = {
                                    let state = app.state::<AppState>();
                                    capture_now(state.inner())
                                };
                                match result {
                                    Ok(view) => {
                                        let _ = app.emit("capture-ok", view);
                                    }
                                    Err(msg) => {
                                        let _ = app.emit("capture-err", msg);
                                    }
                                }
                                if let Some(w) = app.get_webview_window("panel") {
                                    let _ = w.set_focus();
                                }
                            });
                        }
                    }
                })
                .build();
            if let Err(e) = app.handle().plugin(plugin) {
                eprintln!("[ubilink] global-shortcut plugin failed to load: {e}");
            } else if let Err(e) = app.global_shortcut().register(toggle) {
                eprintln!("[ubilink] couldn't register Cmd+Shift+K (grant Accessibility permission): {e}");
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
            open_settings,
            request_accessibility
        ])
        .run(tauri::generate_context!())
        .expect("error while running ubilink desktop");
}
