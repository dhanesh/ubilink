//! The GPUI shell (R4): tray item, three global hotkeys, one pop-up panel with a
//! blurred background on the display under the cursor, accessory activation policy —
//! all in one process. See docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md
//! (R4, TN4, D5).
//!
//! Threads and the event loop (TN4): GPUI owns the main thread's NSApp loop. tray-icon
//! and global-hotkey are created on that thread inside `Application::run` (by `start`)
//! and live in the `Shell` global for the app's lifetime. Their events arrive in
//! crossbeam channels that nothing wakes GPUI for, so a GPUI foreground task polls them
//! every `POLL` (`pump`). The only other thread is the single-instance listener, which
//! feeds an mpsc channel the same pump drains. Spec D5's fallback (an objc2 NSStatusItem
//! and a Carbon RegisterEventHotKey shim) was not needed: both crates work in GPUI's
//! loop — see the Task 4 report for the live evidence.

pub mod hotkeys;
pub mod native;
pub mod panel;
pub mod placement;
pub mod single_instance;
pub mod state;
pub mod tray;

use std::sync::mpsc::Receiver;
use std::time::Duration;

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;
use objc2::MainThreadMarker;
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIcon, TrayIconEvent};

pub use state::ShellState;

/// Event-poll interval. 16 ms keeps a hotkey's poll delay under one 60 Hz frame (R5's
/// p50 < 80 ms budget has room for it) at the price of ~60 wakeups a second while idle;
/// the Task 4 report measures the resulting idle CPU.
const POLL: Duration = Duration::from_millis(16);

/// What the panel shows. Task 13's router turns each into a real view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Capture,
    Browser,
    Settings,
    // Opened by Task 13's first-run flow; nothing in the shell opens it yet.
    #[allow(dead_code)]
    Onboarding,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Capture => "capture",
            Route::Browser => "browser",
            Route::Settings => "settings",
            Route::Onboarding => "onboarding",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Route::Capture => "Capture",
            Route::Browser => "Browse links",
            Route::Settings => "Settings",
            Route::Onboarding => "Welcome to ubilink",
        }
    }

    /// R5: capture and browser hide on loss of key status; settings and onboarding are
    /// places the user works in (and may switch apps from, to grant a permission), so
    /// they stay open.
    pub fn hides_on_deactivate(self) -> bool {
        matches!(self, Route::Capture | Route::Browser)
    }
}

/// What the tray, hotkeys and single-instance socket ask the shell to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the panel on this route (switching route if it is already open).
    Open(Route),
    /// Hotkeys: hide if open on this route, otherwise `Open`.
    Toggle(Route),
    Quit,
}

struct Panel {
    handle: AnyWindowHandle,
    view: Entity<panel::PanelView>,
    route: Route,
}

/// Owns the OS integrations for the app's lifetime: dropping the tray removes the
/// status item and dropping the manager unregisters the hotkeys.
pub struct Shell {
    state: ShellState,
    tray: Option<TrayIcon>,
    _hotkeys: Option<GlobalHotKeyManager>,
    hotkey_routes: Vec<(u32, Route)>,
    instance_rx: Option<Receiver<()>>,
    panel: Option<Panel>,
}

impl Global for Shell {}

/// Create the tray and hotkeys and start the event pump. Call once, inside GPUI's run
/// closure (main thread, NSApp loop running), after the accessory policy is set.
pub fn start(cx: &mut App, instance_rx: Option<Receiver<()>>) {
    let mut state = ShellState {
        window_kind: panel::REQUESTED_KIND.into(),
        background: panel::REQUESTED_BACKGROUND.into(),
        ..ShellState::default()
    };

    let tray = match tray::build() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("[ubilink-next] tray item unavailable: {e}");
            None
        }
    };

    let reg = hotkeys::register();
    state.hotkeys_registered = reg.report.registered;
    state.hotkeys_skipped = reg.report.skipped;
    state.hotkey_error = reg.report.error;

    cx.set_global(Shell {
        state,
        tray,
        _hotkeys: reg.manager,
        hotkey_routes: reg.routes,
        instance_rx,
        panel: None,
    });
    log_state(cx);

    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(POLL).await;
        cx.update(pump);
    })
    .detach();
}

/// The shell's state with the live values re-read: tray visibility and the activation
/// policy from AppKit. Task 6's automation `state` verb returns this.
pub fn state(cx: &mut App) -> ShellState {
    let Some(shell) = cx.try_global::<Shell>() else {
        return ShellState::default();
    };
    let mut s = shell.state.clone();
    s.tray_present = shell.tray.as_ref().is_some_and(tray::present);
    if let Some(mtm) = MainThreadMarker::new() {
        s.record_policy(native::activation_policy(mtm));
    }
    s
}

/// One line of JSON on stderr whenever the state changes, so a maintainer running the
/// binary from a terminal sees what the probe will see.
fn log_state(cx: &mut App) {
    let s = state(cx);
    if let Ok(json) = serde_json::to_string(&s) {
        eprintln!("[ubilink-next] shell state: {json}");
    }
}

/// Drain every event channel, then act. Commands are collected first so no global
/// borrow is held while windows open or close.
fn pump(cx: &mut App) {
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let mut cmds = Vec::new();
    while let Ok(e) = GlobalHotKeyEvent::receiver().try_recv() {
        if e.state() == HotKeyState::Pressed {
            if let Some((_, r)) = shell.hotkey_routes.iter().find(|(id, _)| *id == e.id()) {
                cmds.push(Command::Toggle(*r));
            }
        }
    }
    while let Ok(e) = MenuEvent::receiver().try_recv() {
        cmds.extend(tray::command_for(e.id().as_ref()));
    }
    // Clicks on the item itself open its menu natively; drain so the channel can't grow.
    while TrayIconEvent::receiver().try_recv().is_ok() {}
    if let Some(rx) = &shell.instance_rx {
        while rx.try_recv().is_ok() {
            cmds.push(Command::Open(Route::Capture));
        }
    }
    for c in cmds {
        execute(c, cx);
    }
}

pub fn execute(cmd: Command, cx: &mut App) {
    // A reopen event can arrive before `start` ran; there is nothing to show yet.
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let current = shell.panel.as_ref().map(|p| p.route);
    match (cmd, current) {
        (Command::Quit, _) => cx.quit(),
        (Command::Toggle(r), Some(open)) if open == r => hide(cx),
        (Command::Open(r) | Command::Toggle(r), Some(_)) => switch_route(r, cx),
        (Command::Open(r) | Command::Toggle(r), None) => open(r, cx),
    }
}

fn open(route: Route, cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (displays, primary_height) = native::displays(mtm);
    let placement =
        placement::place_panel(&displays, native::cursor(primary_height), panel::PANEL_SIZE);
    let opened = gpui_kit::open_window(panel::options(placement), cx, |window, cx| {
        cx.new(|cx| panel::PanelView::new(route, window, cx))
    });
    let (handle, view) = match opened {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[ubilink-next] panel failed to open: {e}");
            return;
        }
    };
    // Read back what AppKit actually built (the window is ordered front synchronously
    // by GPUI's open), so the state reports facts rather than the request.
    let facts = native::inspect_panel(mtm);
    cx.update_global::<Shell, _>(|shell, _| {
        shell.panel = Some(Panel {
            handle,
            view,
            route,
        });
        let s = &mut shell.state;
        s.visible = true;
        s.route = Some(route.as_str().into());
        s.display_id = placement.map(|p| p.display_id);
        if let Some(f) = facts {
            s.window_kind = if f.popup { "popup" } else { "other" }.into();
            s.background = if f.blurred { "blurred" } else { "opaque" }.into();
            s.window_verified = true;
        }
    });
    log_state(cx);
}

fn switch_route(route: Route, cx: &mut App) {
    let Some(view) = cx.update_global::<Shell, _>(|shell, _| {
        shell.panel.as_mut().map(|p| {
            p.route = route;
            p.view.clone()
        })
    }) else {
        return;
    };
    view.update(cx, |v, cx| v.set_route(route, cx));
    cx.update_global::<Shell, _>(|shell, _| shell.state.route = Some(route.as_str().into()));
    log_state(cx);
}

fn hide(cx: &mut App) {
    let Some(p) = cx.update_global::<Shell, _>(|shell, _| shell.panel.take()) else {
        return;
    };
    let _ = p.handle.update(cx, |_, window, _| window.remove_window());
    mark_hidden(cx);
}

/// The panel closed itself (click-away). Only forget it if it is still the current
/// panel: a late deactivation from an old window must not drop a newer one.
pub fn panel_closed(id: WindowId, cx: &mut App) {
    let was_current = cx.update_global::<Shell, _>(|shell, _| {
        let current = shell
            .panel
            .as_ref()
            .is_some_and(|p| p.handle.window_id() == id);
        if current {
            shell.panel = None;
        }
        current
    });
    if was_current {
        mark_hidden(cx);
    }
}

fn mark_hidden(cx: &mut App) {
    cx.update_global::<Shell, _>(|shell, _| {
        shell.state.visible = false;
        shell.state.route = None;
    });
    log_state(cx);
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that glob carries gpui-kit's re-exported `test` macro, which
    // would shadow Rust's `#[test]`.
    use super::Route;

    #[test]
    fn capture_and_browser_hide_on_deactivate_settings_and_onboarding_stay() {
        assert!(Route::Capture.hides_on_deactivate());
        assert!(Route::Browser.hides_on_deactivate());
        assert!(!Route::Settings.hides_on_deactivate());
        assert!(!Route::Onboarding.hides_on_deactivate());
    }
}
