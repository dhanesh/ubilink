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

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use ubilink_core::{
    AdapterRegistry, BrowseEntry, CommandRunner, ConfiguredAdapter, LinkGraph, OsaScriptRunner,
    PanelBackend, PanelView, PermissionState, Support, UbilinkService, FRONTMOST_APP_SCRIPT,
};

/// App state: the link-graph-backed panel backend + the declarative adapter registry.
struct AppState {
    backend: Mutex<PanelBackend>,
    adapters: AdapterRegistry,
    /// App names (lowercased) of third-party plugins the user has turned OFF.
    /// Built-in adapters can't be disabled, so they never appear here.
    disabled: Mutex<std::collections::HashSet<String>>,
    /// Where plugins-state.json lives (so toggles persist across launches).
    data_dir: std::path::PathBuf,
}

/// One row in the plugin manager: a supported app, whether it's a built-in
/// default-macOS adapter or a third-party plugin, and whether it's enabled.
#[derive(serde::Serialize, Clone)]
struct PluginView {
    app: String,
    builtin: bool,
    enabled: bool,
    scheme: Option<String>,
}

/// Read the persisted set of disabled plugin app-names (lowercased). Missing/garbled
/// file → empty (everything enabled). Tiny std-only JSON via serde_json.
fn load_disabled(data_dir: &std::path::Path) -> std::collections::HashSet<String> {
    let path = data_dir.join("plugins-state.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Default::default();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Default::default();
    };
    v.get("disabled")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// Persist the disabled set. Best-effort (a failed write just means the toggle
/// won't survive a restart — surfaced as the command's error).
fn save_disabled(
    data_dir: &std::path::Path,
    disabled: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let mut list: Vec<&String> = disabled.iter().collect();
    list.sort();
    let body = serde_json::json!({ "disabled": list });
    std::fs::write(data_dir.join("plugins-state.json"), body.to_string()).map_err(|e| e.to_string())
}

/// Capture the resource of whatever app is frontmost RIGHT NOW: detect it, look up
/// its declarative adapter (adapters_config), capture+validate+surface-backlinks.
/// Called from the hotkey handler (before the panel steals focus) and the
/// `capture_current` command. U3 explains an unsupported app instead of failing.
fn capture_now(state: &AppState) -> Result<PanelView, String> {
    let frontmost = OsaScriptRunner
        .run("osascript", &["-e", FRONTMOST_APP_SCRIPT])
        .map_err(|e| format!("Couldn't tell which app is active: {e}"))?;
    let spec = state.adapters.for_app(&frontmost).ok_or_else(|| {
        format!(
            "Linking from \u{201c}{}\u{201d} isn't supported yet.",
            frontmost.trim()
        )
    })?;
    // Honor the plugin manager: a third-party plugin the user turned OFF behaves as
    // unsupported (built-ins can't be disabled, so they always pass).
    if !spec.builtin {
        let disabled = state
            .disabled
            .lock()
            .map_err(|_| "internal state was poisoned".to_string())?;
        if disabled.contains(&spec.app.to_ascii_lowercase()) {
            return Err(format!(
                "Linking from \u{201c}{}\u{201d} is turned off in Settings.",
                spec.app
            ));
        }
    }
    let adapter = ConfiguredAdapter::new(spec.clone(), OsaScriptRunner);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut b = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    b.capture_current(&adapter, now)
}

#[tauri::command]
fn capture_current(state: State<'_, AppState>) -> Result<PanelView, String> {
    capture_now(state.inner())
}

/// The link-browser top level (Cmd+Shift+L): every captured resource, title+uri.
#[tauri::command]
fn list_links(state: State<'_, AppState>) -> Result<Vec<BrowseEntry>, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.list_links()
}

/// Drill-in (→): everything linked to `uri`.
#[tauri::command]
fn links_of(state: State<'_, AppState>, uri: String) -> Result<Vec<BrowseEntry>, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.links_of(&uri)
}

/// Link two captured resources (browser "L"). The core has no clock, so the
/// desktop supplies the timestamp.
#[tauri::command]
fn link_resources(state: State<'_, AppState>, a_uri: String, b_uri: String) -> Result<(), String> {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64);
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.link_resources(&a_uri, &b_uri, created_at)
}

/// Settings: resource + link counts.
#[tauri::command]
fn graph_stats(state: State<'_, AppState>) -> Result<ubilink_core::GraphStats, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.graph_stats()
}

/// Settings: wipe the graph (destructive — the webview confirms first). Returns
/// the number of resources removed.
#[tauri::command]
fn reset_graph(state: State<'_, AppState>) -> Result<i64, String> {
    let mut backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.reset_graph()
}

/// Settings: export the graph as JSON to ~/Downloads, returning the written path.
#[tauri::command]
fn export_graph(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let json = {
        let backend = state
            .backend
            .lock()
            .map_err(|_| "internal state was poisoned".to_string())?;
        backend.export_graph()?
    };
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("couldn't resolve a save directory: {e}"))?;
    // No clock in core; stamp the filename from the OS time here.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("ubilink-export-{stamp}.json"));
    std::fs::write(&path, json).map_err(|e| format!("couldn't write export: {e}"))?;
    let p = path.to_string_lossy().to_string();
    // Reveal it in Finder so the user sees where it went.
    let _ = std::process::Command::new("open").arg("-R").arg(&p).spawn();
    Ok(p)
}

/// Settings: import a graph-export JSON (read by the webview's file input).
/// Merges into the current graph; returns a human summary.
#[tauri::command]
fn import_graph(state: State<'_, AppState>, json: String) -> Result<String, String> {
    let mut backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    backend.import_graph(&json)
}

/// In-app capability disclosure (O2): what linking works where.
#[tauri::command]
fn capabilities(state: State<'_, AppState>) -> Result<Vec<Support>, String> {
    let backend = state
        .backend
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    Ok(backend.capabilities())
}

/// Settings › Plugins: every adapter as a toggle row (built-in vs third-party,
/// enabled/disabled). Sorted case-insensitively by app name for a stable list.
#[tauri::command]
fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginView>, String> {
    let disabled = state
        .disabled
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    let mut rows: Vec<PluginView> = state
        .adapters
        .specs()
        .iter()
        .map(|s| PluginView {
            app: s.app.clone(),
            builtin: s.builtin,
            // Built-ins are always on; a third-party plugin is on unless the user disabled it.
            enabled: s.builtin || !disabled.contains(&s.app.to_ascii_lowercase()),
            scheme: s.schemes.first().cloned(),
        })
        .collect();
    rows.sort_by_key(|p| p.app.to_ascii_lowercase());
    Ok(rows)
}

/// Settings › Plugins: toggle a third-party plugin on/off and persist it. Built-in
/// adapters (core macOS apps ubilink ships with) can't be disabled.
#[tauri::command]
fn set_plugin_enabled(
    state: State<'_, AppState>,
    app: String,
    enabled: bool,
) -> Result<(), String> {
    let is_builtin = state
        .adapters
        .specs()
        .iter()
        .any(|s| s.app.eq_ignore_ascii_case(&app) && s.builtin);
    if is_builtin && !enabled {
        return Err(format!(
            "{app} is a built-in adapter and can't be disabled."
        ));
    }
    let key = app.to_ascii_lowercase();
    let mut disabled = state
        .disabled
        .lock()
        .map_err(|_| "internal state was poisoned".to_string())?;
    if enabled {
        disabled.remove(&key);
    } else {
        disabled.insert(key);
    }
    save_disabled(&state.data_dir, &disabled)
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

/// Resize the panel window to a content-fit height (the webview measures/computes
/// it). Done in Rust because the JS window API proved unreliable under the
/// transparent window; `invoke` + `set_size` are the proven path.
#[tauri::command]
fn resize_panel(app: AppHandle, height: f64) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("panel") {
        let h = height.clamp(96.0, 900.0);
        let _ = w.set_size(tauri::LogicalSize::new(640.0_f64, h));
    }
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
    PermissionReport {
        accessibility: accessibility_trusted(),
        automation,
    }
}

/// Open the relevant System Settings privacy pane so the user can grant a permission.
#[tauri::command]
fn open_settings(pane: String) -> Result<(), String> {
    let url = match pane.as_str() {
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        "automation" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"
        }
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
            // Which third-party plugins has the user turned off? Persisted across launches.
            let disabled = load_disabled(&data_dir);
            app.manage(AppState {
                backend: Mutex::new(PanelBackend::new(
                    UbilinkService::new(graph).with_permission(permission).allow_schemes(schemes),
                )),
                adapters,
                disabled: Mutex::new(disabled),
                data_dir,
            });

            // U1 render-first: run as an Accessory app (overlay-style, like Spotlight/
            // Raycast). Showing the panel then does NOT make ubilink the frontmost
            // *application*, so the async capture below still sees the app you were in
            // — which lets us paint the panel BEFORE the slow osascript capture runs.
            // Trade-off: no Dock icon (summon with the hotkey; Esc dismisses).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Aurora material: a native NSVisualEffectView (HUD vibrancy) behind the
            // transparent webview, rounded to the panel radius (17px). The CSS panel
            // surface (--ui-bg, a translucent tint) sits on top of this real blur.
            #[cfg(target_os = "macos")]
            if let Some(w) = app.get_webview_window("panel") {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
                let _ = apply_vibrancy(
                    &w,
                    NSVisualEffectMaterial::HudWindow,
                    Some(NSVisualEffectState::Active),
                    Some(17.0),
                );
            }

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
            // Cmd+Shift+L opens the link-browser (K = capture, L = browse).
            let browse = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
            let browse_for_handler = browse;
            // Cmd+Shift+Comma opens Settings (⌘, is reserved by macOS for the menu).
            let settings = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Comma);
            let settings_for_handler = settings;
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
                    } else if shortcut == &browse_for_handler && event.state() == ShortcutState::Pressed {
                        if let Some(w) = app.get_webview_window("panel") {
                            // Cmd+Shift+L: open the link-browser. Graph reads are local
                            // SQLite (fast), so list synchronously, push it, and reveal.
                            let listed = {
                                let state = app.state::<AppState>();
                                let backend = state.backend.lock();
                                match backend {
                                    Ok(b) => b.list_links(),
                                    Err(_) => Err("internal state was poisoned".to_string()),
                                }
                            };
                            match listed {
                                Ok(list) => {
                                    let _ = w.emit("browse", list);
                                }
                                Err(msg) => {
                                    let _ = w.emit("browse-err", msg);
                                }
                            }
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    } else if shortcut == &settings_for_handler && event.state() == ShortcutState::Pressed {
                        if let Some(w) = app.get_webview_window("panel") {
                            // Cmd+Shift+, : open Settings. Push the current graph stats.
                            let stats = {
                                let state = app.state::<AppState>();
                                let backend = state.backend.lock();
                                match backend {
                                    Ok(b) => b.graph_stats().ok(),
                                    Err(_) => None,
                                }
                            };
                            let _ = w.emit("settings", stats);
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build();
            if let Err(e) = app.handle().plugin(plugin) {
                eprintln!("[ubilink] global-shortcut plugin failed to load: {e}");
            } else {
                if let Err(e) = app.global_shortcut().register(toggle) {
                    eprintln!("[ubilink] couldn't register Cmd+Shift+K (grant Accessibility permission): {e}");
                }
                if let Err(e) = app.global_shortcut().register(browse) {
                    eprintln!("[ubilink] couldn't register Cmd+Shift+L (browse): {e}");
                }
                if let Err(e) = app.global_shortcut().register(settings) {
                    eprintln!("[ubilink] couldn't register Cmd+Shift+, (settings): {e}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            capture_current,
            list_links,
            links_of,
            link_resources,
            resize_panel,
            graph_stats,
            reset_graph,
            export_graph,
            import_graph,
            capabilities,
            list_plugins,
            set_plugin_enabled,
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

#[cfg(test)]
mod tests {
    use super::{load_disabled, save_disabled};
    use std::collections::HashSet;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        // Unique per test (name + pid) so parallel test threads don't clobber each other.
        let d = std::env::temp_dir().join(format!("ubilink-plugins-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn disabled_set_round_trips() {
        let dir = tmp_dir("roundtrip");
        let mut set = HashSet::new();
        set.insert("bear".to_string());
        set.insert("things3".to_string());
        save_disabled(&dir, &set).unwrap();
        let back = load_disabled(&dir);
        assert_eq!(back, set);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_empty() {
        // A never-written data dir → nothing disabled (everything enabled).
        let dir = std::env::temp_dir().join("ubilink-plugins-nonexistent-xyz");
        std::fs::remove_dir_all(&dir).ok();
        assert!(load_disabled(&dir).is_empty());
    }

    #[test]
    fn garbled_file_is_empty() {
        // A corrupt plugins-state.json must not panic — degrade to "all enabled".
        let dir = tmp_dir("garbled");
        std::fs::write(dir.join("plugins-state.json"), b"{ not json").unwrap();
        assert!(load_disabled(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
