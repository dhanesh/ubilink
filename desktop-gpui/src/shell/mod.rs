//! The GPUI shell (R4): tray item, three global hotkeys, one pop-up panel with a
//! blurred background on the display under the cursor, accessory activation policy —
//! all in one process. See docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md
//! (R4, TN4, D5).
//!
//! Threads and the event loop (TN4): GPUI owns the main thread's NSApp loop. tray-icon
//! and global-hotkey are created on that thread inside `Application::run` (by `start`)
//! and live in the `Shell` global for the app's lifetime. Their event handlers (set in
//! `start`), plus the single-instance listener thread (the only other thread), post
//! `ShellEvent`s to one `Inbox`: an mpsc channel and a `wake::Signal`. A GPUI foreground
//! task awaits the signal and then drains the channel (`pump`). Nothing runs while
//! idle: no timer and no poll (Phase 1 O1 idle-CPU budget). Spec D5's fallback (an objc2 NSStatusItem
//! and a Carbon RegisterEventHotKey shim) was not needed: both crates work in GPUI's
//! loop — see the Task 4 report for the live evidence.

pub mod hotkeys;
pub mod native;
pub mod panel;
pub mod placement;
pub mod single_instance;
pub mod state;
pub mod tray;
pub mod wake;

use std::sync::mpsc::{self, Receiver, Sender};

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;
use objc2::MainThreadMarker;
use tray_icon::menu::MenuEvent;
use tray_icon::{TrayIcon, TrayIconEvent};

pub use state::ShellState;

/// Everything that can ask the shell to act, from any thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    HotKey {
        id: u32,
        pressed: bool,
    },
    Menu(String),
    /// A second instance (single-instance socket) asked for the panel.
    Show,
}

/// The sending side of the shell's event channel: post an event, then wake the pump.
/// `Sender` is Send + Sync, so tray-icon and global-hotkey's `Fn + Send + Sync`
/// handlers can own one.
#[derive(Clone)]
pub struct Inbox {
    tx: Sender<ShellEvent>,
    signal: wake::Signal,
}

impl Inbox {
    pub fn post(&self, event: ShellEvent) {
        // A send error means the app is shutting down; there is nothing to wake.
        if self.tx.send(event).is_ok() {
            self.signal.notify();
        }
    }
}

/// Create the inbox before GPUI starts, so the single-instance listener can post to it.
pub fn inbox() -> (Inbox, Receiver<ShellEvent>) {
    let (tx, rx) = mpsc::channel();
    (
        Inbox {
            tx,
            signal: wake::Signal::default(),
        },
        rx,
    )
}

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
    events: Receiver<ShellEvent>,
    panel: Option<Panel>,
}

impl Global for Shell {}

/// Create the tray and hotkeys and start the event pump. Call once, inside GPUI's run
/// closure (main thread, NSApp loop running), after the accessory policy is set.
pub fn start(cx: &mut App, inbox: Inbox, events: Receiver<ShellEvent>) {
    // Event-driven, not polled: with a handler set, each crate calls it instead of
    // queueing on its own channel. Both fire on the main thread (Carbon hotkey handler,
    // NSMenu action); `post` wakes the pump through GPUI's waker either way.
    let hk = inbox.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        hk.post(ShellEvent::HotKey {
            id: e.id(),
            pressed: e.state() == HotKeyState::Pressed,
        })
    }));
    let menu = inbox.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        menu.post(ShellEvent::Menu(e.id().as_ref().to_string()))
    }));
    // Clicks on the item itself open its menu natively. A no-op handler stops
    // tray-icon queueing every click on a channel nobody reads.
    TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));

    let mut state = startup_state();

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
        events,
        panel: None,
    });
    log_state(cx);

    let signal = inbox.signal.clone();
    cx.spawn(async move |cx| loop {
        signal.wait().await;
        cx.update(pump);
    })
    .detach();
}

/// The state before anything is observed. `window_kind`/`background` stay "unknown"
/// until a live panel is read back, so a shell that never opened a window cannot pass
/// R4's popup/blurred check.
fn startup_state() -> ShellState {
    ShellState::default()
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
    let cmds: Vec<Command> = shell
        .events
        .try_iter()
        .filter_map(|e| command_for(&e, &shell.hotkey_routes))
        .collect();
    for c in cmds {
        execute(c, cx);
    }
}

/// Pure: what an event asks for. Only hotkey presses act (a release would toggle the
/// panel straight back).
fn command_for(event: &ShellEvent, hotkey_routes: &[(u32, Route)]) -> Option<Command> {
    match event {
        ShellEvent::HotKey { id, pressed: true } => hotkey_routes
            .iter()
            .find(|(hid, _)| hid == id)
            .map(|(_, r)| Command::Toggle(*r)),
        ShellEvent::HotKey { pressed: false, .. } => None,
        ShellEvent::Menu(id) => tray::command_for(id),
        ShellEvent::Show => Some(Command::Open(Route::Capture)),
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
            s.record_panel(f);
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
    use super::{command_for, startup_state, Command, Route, ShellEvent};

    #[test]
    fn startup_state_claims_no_window_kind_or_background() {
        let s = startup_state();
        assert_eq!(s.window_kind, "unknown");
        assert_eq!(s.background, "unknown");
        assert!(!s.window_verified);
        assert!(!s.visible);
    }

    #[test]
    fn events_map_to_commands_presses_only() {
        let routes = [(7, Route::Capture), (8, Route::Settings)];
        let press = |id| ShellEvent::HotKey { id, pressed: true };
        assert_eq!(
            command_for(&press(7), &routes),
            Some(Command::Toggle(Route::Capture))
        );
        assert_eq!(
            command_for(&press(8), &routes),
            Some(Command::Toggle(Route::Settings))
        );
        assert_eq!(command_for(&press(9), &routes), None, "unknown id");
        let release = ShellEvent::HotKey {
            id: 7,
            pressed: false,
        };
        assert_eq!(command_for(&release, &routes), None);
        assert_eq!(
            command_for(&ShellEvent::Menu("quit".into()), &routes),
            Some(Command::Quit)
        );
        assert_eq!(
            command_for(&ShellEvent::Show, &routes),
            Some(Command::Open(Route::Capture))
        );
    }

    #[test]
    fn capture_and_browser_hide_on_deactivate_settings_and_onboarding_stay() {
        assert!(Route::Capture.hides_on_deactivate());
        assert!(Route::Browser.hides_on_deactivate());
        assert!(!Route::Settings.hides_on_deactivate());
        assert!(!Route::Onboarding.hides_on_deactivate());
    }
}
