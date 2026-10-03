// Satisfies: T1 (broker/core wired into Tauri commands), U1 (global hotkey -> panel,
// capturing the app you were IN, not the panel itself), U2 (copy in <=2 actions), U3
// (explained errors + permission onboarding), U5 (backlinks), O2 (capability
// disclosure), S4/RT-2 (permission detection + guided granting).
// See docs/RUNBOOK-gui-panel.md.
//
// The imperative GUI shell. Logic lives in ubilink-core (headlessly tested) AND, as
// of R3/task 3 of the GPUI Phase 0 plan, in ubilink-app — the GUI-free command layer
// shared with the future GPUI shell (desktop-gpui/). Every function below is either
// a thin `#[tauri::command]` wrapper that hands ubilink-app's functions the explicit
// inputs they need (an `&AppState`, a resolved save directory, …) in place of
// `tauri::State`/`AppHandle`, or a genuinely Tauri-specific piece — window geometry,
// `restart_app`, the global-shortcut registration path — that has no GUI-free
// equivalent to delegate to. See task-3-report.md for the full move/stay inventory.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod placement;

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use ubilink_app::{AppState, HotkeyReport, PermissionReport, PluginView};
use ubilink_core::{
    AdapterRegistry, BrowseEntry, CapabilityMatrix, LinkGraph, OsaScriptRunner, PanelBackend,
    PanelView, Support, UbilinkService,
};

#[tauri::command]
fn capture_current(state: State<'_, AppState>) -> Result<PanelView, String> {
    ubilink_app::capture_now(state.inner())
}

/// The link-browser top level (Cmd+Shift+L): every captured resource, title+uri.
#[tauri::command]
fn list_links(state: State<'_, AppState>) -> Result<Vec<BrowseEntry>, String> {
    ubilink_app::list_links(state.inner())
}

/// Drill-in (→): everything linked to `uri`.
#[tauri::command]
fn links_of(state: State<'_, AppState>, uri: String) -> Result<Vec<BrowseEntry>, String> {
    ubilink_app::links_of(state.inner(), &uri)
}

/// Link two captured resources (browser "L").
#[tauri::command]
fn link_resources(state: State<'_, AppState>, a_uri: String, b_uri: String) -> Result<(), String> {
    ubilink_app::link_resources(state.inner(), &a_uri, &b_uri)
}

/// Settings: resource + link counts.
#[tauri::command]
fn graph_stats(state: State<'_, AppState>) -> Result<ubilink_core::GraphStats, String> {
    ubilink_app::graph_stats(state.inner())
}

/// Settings: wipe the graph (destructive — the webview confirms first). Returns
/// the number of resources removed.
#[tauri::command]
fn reset_graph(state: State<'_, AppState>) -> Result<i64, String> {
    ubilink_app::reset_graph(state.inner())
}

/// Settings: export the graph as JSON to ~/Downloads, returning the written path.
/// The save-directory lookup needs `AppHandle` (genuinely Tauri), so it stays here;
/// everything else (reading the graph, stamping the filename, writing the file) is
/// ubilink-app's `export_graph`.
#[tauri::command]
fn export_graph(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("couldn't resolve a save directory: {e}"))?;
    let path = ubilink_app::export_graph(state.inner(), &dir)?;
    // Reveal it in Finder so the user sees where it went.
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn();
    Ok(path)
}

/// Settings: import a graph-export JSON (read by the webview's file input).
/// Merges into the current graph; returns a human summary.
#[tauri::command]
fn import_graph(state: State<'_, AppState>, json: String) -> Result<String, String> {
    ubilink_app::import_graph(state.inner(), &json)
}

/// In-app capability disclosure (O2): what linking works where.
#[tauri::command]
fn capabilities(state: State<'_, AppState>) -> Result<Vec<Support>, String> {
    ubilink_app::capabilities(state.inner())
}

/// Settings › App adapters: every adapter as a toggle row.
#[tauri::command]
fn list_plugins(state: State<'_, AppState>) -> Result<Vec<PluginView>, String> {
    ubilink_app::list_plugins(state.inner())
}

/// Settings › App adapters: toggle a non-built-in adapter on/off and persist it.
#[tauri::command]
fn set_plugin_enabled(
    state: State<'_, AppState>,
    app: String,
    enabled: bool,
) -> Result<(), String> {
    ubilink_app::set_plugin_enabled(state.inner(), &app, enabled)
}

/// U2: put the link on the clipboard.
#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    ubilink_app::copy_to_clipboard(&text)
}

/// Open a captured link's URI.
#[tauri::command]
fn open_link(state: State<'_, AppState>, uri: String) -> Result<(), String> {
    ubilink_app::open_link(state.inner(), &uri)
}

/// Resize the panel window to a content-fit height (the webview measures/computes
/// it). Done in Rust because the JS window API proved unreliable under the
/// transparent window; `invoke` + `set_size` are the proven path. Kept in the shell
/// (not ubilink-app): it needs a live `AppHandle`/`WebviewWindow` to move/resize an
/// actual OS window — there is no GUI-free equivalent to delegate to. The one pure
/// piece it leans on, the vertical-fit math, already lives in `placement` (unit-
/// tested there), which is as far as this command's logic can be factored out.
#[tauri::command]
fn resize_panel(app: AppHandle, height: f64) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("panel") {
        let mut h = height.clamp(96.0, 900.0);
        // Keep the panel on its display as the content grows: cap the height to
        // the usable area (the panel scrolls inside) and move it up if it would
        // run past the bottom. Settings used to extend off-screen, unreachable.
        let fitted = (|| {
            let scale = w.scale_factor().ok()?;
            let pos = w.outer_position().ok()?.to_logical::<f64>(scale);
            let work = display_of(&w.current_monitor().ok()??).work;
            Some((pos.x, placement::fit_vertically(work, pos.y, h)))
        })();
        if let Some((x, (top, fitted_h))) = fitted {
            h = fitted_h;
            let _ = w.set_position(tauri::LogicalPosition::new(x, top));
        }
        let _ = w.set_size(tauri::LogicalSize::new(640.0_f64, h));
    }
    Ok(())
}

/// Move the panel onto the display under the mouse cursor, so it opens where the
/// user is looking rather than on whichever display it was created on. The maths
/// (and the macOS coordinate-space conversions it depends on) live in
/// `placement`, where they're unit-tested. Best effort: if any query fails the
/// window stays where it was, which is the old behaviour.
fn place_panel(app: &AppHandle, w: &tauri::WebviewWindow) {
    let (Ok(cursor), Ok(monitors)) = (app.cursor_position(), app.available_monitors()) else {
        return;
    };
    let primary = app.primary_monitor().ok().flatten();
    // Primary first: it's where the panel goes if the cursor is on no display.
    let mut displays: Vec<placement::Display> = primary.iter().map(display_of).collect();
    for m in &monitors {
        let d = display_of(m);
        if !displays.contains(&d) {
            displays.push(d);
        }
    }
    let (Ok(size), Ok(scale)) = (w.outer_size(), w.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    // tao reports the cursor scaled by the PRIMARY display's factor.
    let primary_scale = primary.as_ref().map_or(1.0, |m| m.scale_factor());
    let cursor = placement::cursor_points(cursor.x, cursor.y, primary_scale);
    if let Some((x, y)) = placement::panel_origin(&displays, cursor, (size.width, size.height)) {
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
}

/// A monitor's frame and usable area in global points. On macOS both are reported
/// as points x that monitor's own scale factor, so both convert the same way.
fn display_of(m: &tauri::Monitor) -> placement::Display {
    let scale = m.scale_factor();
    let (p, s) = (m.position(), m.size());
    let work = m.work_area();
    placement::Display {
        frame: placement::Screen::from_physical(p.x, p.y, s.width, s.height, scale),
        work: placement::Screen::from_physical(
            work.position.x,
            work.position.y,
            work.size.width,
            work.size.height,
            scale,
        ),
    }
}

/// Hide the panel — the flow is done (U2: 2 actions, then get out of the way). Stays
/// in the shell: hiding an OS window needs a live `AppHandle`, nothing to extract.
#[tauri::command]
fn dismiss_panel(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("panel") {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The three global shortcuts ubilink registers. A single source of truth so both
/// startup registration (`.setup()`) and `retry_hotkeys` build identical values.
/// Stays in the shell: `Shortcut` is a `tauri_plugin_global_shortcut` type, which
/// ubilink-app must never depend on (see its `deps-clean` guard).
fn hotkey_shortcuts() -> (Shortcut, Shortcut, Shortcut) {
    (
        Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyK),
        Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL),
        Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Comma),
    )
}

/// Register the three global shortcuts. IDEMPOTENT: unregisters each first, so
/// calling this again on an already-registered shortcut (Defect 2 — retrying after
/// the user grants Accessibility mid-session) neither errors nor double-fires the
/// handler. Used at startup AND from `retry_hotkeys`. Stays in the shell: it drives
/// `GlobalShortcutExt`, the Tauri plugin API; only the report-folding it feeds into
/// (`build_hotkey_report`) is pure enough to live in ubilink-app.
fn register_hotkeys(
    app: &AppHandle,
    toggle: Shortcut,
    browse: Shortcut,
    settings: Shortcut,
) -> HotkeyReport {
    let gs = app.global_shortcut();
    // Best-effort: an unregistered shortcut errors here, which we ignore — the
    // point is only to clear any prior registration before re-adding it.
    let _ = gs.unregister(toggle);
    let _ = gs.unregister(browse);
    let _ = gs.unregister(settings);
    let report = ubilink_app::build_hotkey_report(
        gs.register(toggle).map_err(|e| e.to_string()),
        gs.register(browse).map_err(|e| e.to_string()),
        gs.register(settings).map_err(|e| e.to_string()),
    );
    if let Some(err) = &report.error {
        eprintln!("[ubilink] hotkey registration issue: {err}");
    }
    report
}

/// Current hotkey registration state, for the onboarding screen.
#[tauri::command]
fn hotkey_status(state: State<'_, AppState>) -> HotkeyReport {
    ubilink_app::hotkey_status(state.inner())
}

/// Re-attempt hotkey registration (Defect 2): registration otherwise only ever runs
/// once, in `.setup()`, so granting Accessibility mid-session never revives a dead
/// hotkey without a full relaunch. Called from the frontend right after the user
/// grants Accessibility (and from Re-check), before it re-reads permission status.
/// Stays in the shell: it drives `register_hotkeys`, which needs `AppHandle`.
#[tauri::command]
fn retry_hotkeys(app: AppHandle) -> HotkeyReport {
    let (toggle, browse, settings) = hotkey_shortcuts();
    let report = register_hotkeys(&app, toggle, browse, settings);
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut guard) = state.hotkeys.lock() {
            *guard = report.clone();
        }
    }
    report
}

/// Show the macOS Accessibility prompt and report whether trust is now granted.
#[tauri::command]
fn request_accessibility() -> bool {
    ubilink_app::request_accessibility()
}

/// Report current permission state (called by the onboarding screen + its Re-check).
#[tauri::command]
fn permission_status() -> PermissionReport {
    ubilink_app::permission_status()
}

/// Open the relevant System Settings privacy pane so the user can grant a permission.
#[tauri::command]
fn open_settings(pane: String) -> Result<(), String> {
    ubilink_app::open_settings(&pane)
}

/// Defect 3: macOS evaluates Accessibility trust at PROCESS LAUNCH, so a
/// process already running when the user grants it in System Settings keeps
/// reading itself as untrusted until it is quit and reopened. Rather than
/// telling the user to do that by hand, offer a button that does it for them.
/// Stays in the shell: `AppHandle::restart` has no GUI-free equivalent.
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
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
                    Ok(n) => eprintln!(
                        "[ubilink] loaded {n} user adapter(s) from {}",
                        user_adapters.display()
                    ),
                    Err(e) => eprintln!(
                        "[ubilink] ignoring invalid {}: {e}",
                        user_adapters.display()
                    ),
                }
            }
            eprintln!(
                "[ubilink] {} app adapter(s) available",
                adapters.apps().len()
            );

            // The bounded API (S5) must trust the schemes the adapters declare
            // (bear:, things:, …) — otherwise a captured bear:// link is rejected.
            let schemes = adapters.declared_schemes();
            // O2: disclose what THIS registry (defaults + user adapters) can link.
            let matrix = CapabilityMatrix::from_registry(&adapters);
            // Which non-built-in adapters has the user turned off? Persisted across launches.
            let disabled = ubilink_app::load_disabled(&data_dir);
            app.manage(AppState {
                backend: Mutex::new(PanelBackend::new(
                    UbilinkService::new(graph)
                        .with_permission(permission)
                        .allow_schemes(schemes)
                        .with_capabilities(matrix),
                )),
                adapters,
                disabled: Mutex::new(disabled),
                data_dir,
                hotkeys: Mutex::new(HotkeyReport::default()),
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
                use window_vibrancy::{
                    apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
                };
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
            // Settings was reachable only by ⌘⇧,, which nothing in the app showed.
            let settings_i =
                MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+Shift+Comma"))?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit ubilink", true, Some("Cmd+Q"))?;
            let tray_menu = Menu::with_items(app, &[&show_i, &settings_i, &quit_i])?;
            let mut tray = TrayIconBuilder::with_id("ubilink-tray")
                .tooltip("Ubilink — ⌘⇧K to capture a link")
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "settings" => {
                        if let Some(w) = app.get_webview_window("panel") {
                            // The same event the ⌘⇧, shortcut sends. With no stats in
                            // the payload, the webview fetches them itself.
                            let _ = w.emit("settings", ());
                            place_panel(app, &w);
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "show" => {
                        if let Some(w) = app.get_webview_window("panel") {
                            place_panel(app, &w);
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
                place_panel(app.handle(), &w);
                let _ = w.show();
                let _ = w.set_focus();
            }

            // U1: the global hotkey CAPTURES the app that is frontmost right now —
            // before we show the panel and steal focus — then pushes the result to
            // the webview and reveals the panel. So it links the app you were IN, and
            // re-captures on every press. (Esc dismisses; see panel.js.)
            // (Cmd+Shift+L opens the link-browser, K = capture, L = browse; Cmd+Shift+Comma
            // opens Settings — ⌘, is reserved by macOS for the app menu.)
            let (toggle, browse, settings) = hotkey_shortcuts();
            let toggle_for_handler = toggle;
            let browse_for_handler = browse;
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
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .ok()
                                    .map(|d| d.as_millis() as u64)
                            } else {
                                None
                            };
                            let _ = w.emit("capturing", t0);
                            place_panel(app, &w);
                            let _ = w.show();

                            // Run the capture OFF the UI path so it never blocks first paint.
                            // As an Accessory app, show() above didn't steal the frontmost
                            // application, so capture_now still detects the app you were in.
                            // When osascript returns, push the result and focus the panel so
                            // Enter (U2) works.
                            let app = app.clone();
                            std::thread::spawn(move || {
                                // Defect 3: the panel was shown (above) but not focused, and
                                // used to stay that way until the WHOLE capture finished —
                                // a window where keystrokes still went to the previous app.
                                // Fix: detect the frontmost app FIRST (capture_now's old first
                                // step), focus the panel immediately, THEN do the slower
                                // per-app work (document-URL scripts, graph writes) with the
                                // app name already in hand. Detection must stay ahead of
                                // set_focus() — reordering it would risk the panel becoming
                                // key window before we know which app to capture from, which
                                // would break capture itself.
                                let result =
                                    ubilink_app::detect_frontmost().and_then(|frontmost| {
                                        if let Some(w) = app.get_webview_window("panel") {
                                            let _ = w.set_focus();
                                        }
                                        let state = app.state::<AppState>();
                                        ubilink_app::capture_from(state.inner(), &frontmost)
                                    });
                                match result {
                                    Ok(view) => {
                                        let _ = app.emit("capture-ok", view);
                                    }
                                    Err(msg) => {
                                        // Detection itself can fail before focus ever happens
                                        // above; make sure the panel is focused regardless so
                                        // U3's error message is legible either way.
                                        if let Some(w) = app.get_webview_window("panel") {
                                            let _ = w.set_focus();
                                        }
                                        let _ = app.emit("capture-err", msg);
                                    }
                                }
                            });
                        }
                    } else if shortcut == &browse_for_handler
                        && event.state() == ShortcutState::Pressed
                    {
                        if let Some(w) = app.get_webview_window("panel") {
                            // Cmd+Shift+L: open the link-browser. Graph reads are local
                            // SQLite (fast), so list synchronously, push it, and reveal.
                            let listed = {
                                let state = app.state::<AppState>();
                                ubilink_app::list_links(state.inner())
                            };
                            match listed {
                                Ok(list) => {
                                    let _ = w.emit("browse", list);
                                }
                                Err(msg) => {
                                    let _ = w.emit("browse-err", msg);
                                }
                            }
                            place_panel(app, &w);
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    } else if shortcut == &settings_for_handler
                        && event.state() == ShortcutState::Pressed
                    {
                        if let Some(w) = app.get_webview_window("panel") {
                            // Cmd+Shift+, : open Settings. Push the current graph stats.
                            let stats = {
                                let state = app.state::<AppState>();
                                ubilink_app::graph_stats(state.inner()).ok()
                            };
                            let _ = w.emit("settings", stats);
                            place_panel(app, &w);
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build();
            if let Err(e) = app.handle().plugin(plugin) {
                eprintln!("[ubilink] global-shortcut plugin failed to load: {e}");
                let report = HotkeyReport {
                    error: Some(format!("global-shortcut plugin failed to load: {e}")),
                    ..HotkeyReport::default()
                };
                if let Ok(mut guard) = app.state::<AppState>().hotkeys.lock() {
                    *guard = report;
                }
            } else {
                // Defect 1: registration failures used to be eprintln!-only, invisible in a
                // packaged .app. Route them through the same report the frontend can query
                // (hotkey_status) and retry (retry_hotkeys) later.
                let report = register_hotkeys(app.handle(), toggle, browse, settings);
                if let Ok(mut guard) = app.state::<AppState>().hotkeys.lock() {
                    *guard = report;
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
            request_accessibility,
            hotkey_status,
            retry_hotkeys,
            restart_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running ubilink desktop");
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_panel_is_allowed_to_start_a_window_drag() {
        // data-tauri-drag-region silently does nothing unless the window's
        // capability grants start-dragging, and core:default does not include it.
        let caps = include_str!("../capabilities/default.json");
        assert!(
            caps.contains("\"core:window:allow-start-dragging\""),
            "capabilities/default.json must allow start-dragging"
        );
    }
}
