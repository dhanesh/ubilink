//! The GPUI shell (R4): tray item, three global hotkeys, one pop-up panel with a
//! blurred background on the display under the cursor, accessory activation policy —
//! all in one process. See docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md
//! (R4, TN4, D5).
//!
//! Threads and the event loop (TN4): GPUI owns the main thread's NSApp loop. tray-icon
//! and global-hotkey are created on that thread inside `Application::run` (by `start`)
//! and live in the `Shell` global for the app's lifetime. Their event handlers (set in
//! `start`), plus the single-instance listener thread (and, in `automation` builds only,
//! the automation socket's threads — R6, see src/automation/), post `ShellEvent`s to
//! one `Inbox`: an mpsc channel and a `wake::Signal`. A GPUI foreground
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

/// What the panel shows. Re-exported from `views` (Task 13's router), which is now the
/// single place a route maps to its view constructor, its view-state and its dismiss
/// policy; the shell only ever matches on the enum itself, never reimplements that
/// table. See `views::Route` and `views::TABLE`.
pub use crate::views::Route;

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
    /// R6: a request from the automation socket, run on the main thread by the pump.
    /// The variant exists only in `automation` builds (S1).
    #[cfg(feature = "automation")]
    Automation(crate::automation::Call),
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

/// What the tray, hotkeys and single-instance socket ask the shell to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the panel on this route (switching route if it is already open).
    Open(Route),
    /// Hotkeys: hide if open on this route, otherwise `Open`.
    Toggle(Route),
    /// Hide the panel, whatever its route, the way a hotkey toggle hides it (Phase 2
    /// R2: the capture view's self-dismiss after a copy). Nothing open: nothing to do.
    Hide,
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
    prewarm(cx);

    let signal = inbox.signal.clone();
    cx.spawn(async move |cx| loop {
        signal.wait().await;
        cx.update(pump);
    })
    .detach();
}

/// U1/R5: open one throwaway panel off-screen at launch, then close it a run-loop turn
/// later, so the user's first hotkey is as fast as every later one.
///
/// Measured (Task 5, debug automation build, 50 triggers): without this the first
/// show took 251–827 ms and every later one 11–50 ms. The first show pays one-off,
/// process-wide costs — `open_window`'s first NSPanel + NSVisualEffectView + Metal
/// layer build (~85 ms vs ~15 ms after) and the main thread's first Core Animation
/// commit for a window (~130–350 ms before the next run-loop task runs) — not
/// per-window ones, so keeping a window alive for reuse would not remove them; doing
/// them once at launch does (first show then 19–141 ms, p99 under U1's 200 ms).
/// The prewarm panel hosts a neutral child (`PanelView::prewarm`), never a route's
/// view, so warming has no side effects once Phase 1's views do real work.
///
/// The panel is placed left of every display, so nothing appears on screen, and is
/// never key (`focus: false`), so no app loses focus. It is not the shell's panel:
/// `Shell::panel` and `ShellState` never see it (visible, window_verified stay false),
/// and `native::visible_panel` prefers the newest panel, so a real show while it is
/// still closing is the one read back. One window, one task: no timer, idle stays 0%.
fn prewarm(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (displays, _) = native::displays(mtm);
    let opened = gpui_kit::open_window(panel::prewarm_options(&displays), cx, |window, cx| {
        cx.new(|cx| panel::PanelView::prewarm(window, cx))
    });
    match opened {
        // The close runs as a foreground task, i.e. after this run-loop turn's Core
        // Animation commit, which is the expensive part being warmed.
        Ok((handle, _)) => cx
            .spawn(async move |cx| {
                let _ = cx.update(|cx| handle.update(cx, |_, window, _| window.remove_window()));
            })
            .detach(),
        Err(e) => eprintln!("[ubilink-next] panel prewarm failed: {e}"),
    }
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

/// Drain every event channel, then act. Events are collected first so no global
/// borrow is held while windows open or close.
fn pump(cx: &mut App) {
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let events: Vec<ShellEvent> = shell.events.try_iter().collect();
    for e in events {
        handle_event(e, cx);
    }
}

/// Act on one event: the pump's per-event step, and the path the automation
/// `trigger_hotkey` verb enters so it runs exactly what a real hotkey runs (R6).
pub fn handle_event(event: ShellEvent, cx: &mut App) {
    #[cfg(feature = "automation")]
    if let ShellEvent::Automation(call) = event {
        crate::automation::handle(call, cx);
        return;
    }
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let Some(c) = command_for(&event, &shell.hotkey_routes) else {
        return;
    };
    // U1/R5: t0 of hotkey-to-first-frame is when the shell receives the press — real
    // Carbon events and `trigger_hotkey` both arrive here. `open` hands it to the new
    // panel, which records the sample at its first presented frame (panel.rs).
    #[cfg(feature = "automation")]
    if let (ShellEvent::HotKey { .. }, Command::Toggle(route)) = (&event, c) {
        crate::automation::store::mark_hotkey(route, std::time::Instant::now());
    }
    execute(c, cx);
}

/// The open panel's window, root view and route, for the automation verbs that act on
/// it (key, resign_key, a11y, state).
#[cfg(feature = "automation")]
pub fn panel(cx: &App) -> Option<(AnyWindowHandle, Entity<panel::PanelView>, Route)> {
    let p = cx.try_global::<Shell>()?.panel.as_ref()?;
    Some((p.handle, p.view.clone(), p.route))
}

/// The global-hotkey id bound to `route`. Known even under `UBILINK_SKIP_HOTKEYS`
/// (ids are computed from the bindings, not from registration), so `trigger_hotkey`
/// works in an automation instance that never grabbed the user's chords (O4).
#[cfg(feature = "automation")]
pub fn hotkey_id_for(route: Route, cx: &App) -> Option<u32> {
    cx.try_global::<Shell>()?
        .hotkey_routes
        .iter()
        .find(|(_, r)| *r == route)
        .map(|(id, _)| *id)
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
        #[cfg(feature = "automation")]
        ShellEvent::Automation(_) => None,
    }
}

pub fn execute(cmd: Command, cx: &mut App) {
    // A reopen event can arrive before `start` ran; there is nothing to show yet.
    let Some(shell) = cx.try_global::<Shell>() else {
        return;
    };
    let current = shell.panel.as_ref().map(|p| p.route);
    // Taken whatever the command does, so a hotkey that hid or switched the panel
    // never leaves a stale t0 for a later, non-hotkey open to be timed against.
    #[cfg(feature = "automation")]
    let hotkey_at = crate::automation::store::take_hotkey_mark().map(|(_, at)| at);
    #[cfg(not(feature = "automation"))]
    let hotkey_at = None;
    match (cmd, current) {
        (Command::Quit, _) => cx.quit(),
        (Command::Hide, _) => hide(cx),
        (Command::Toggle(r), Some(open)) if open == r => hide(cx),
        (Command::Open(r) | Command::Toggle(r), Some(_)) => switch_route(r, cx),
        (Command::Open(r) | Command::Toggle(r), None) => open(r, hotkey_at, cx),
    }
}

/// `hotkey_at`: when the hotkey that asked for this show was received (automation
/// builds only; always `None` otherwise), for R5's latency sample.
fn open(route: Route, hotkey_at: Option<std::time::Instant>, cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (displays, primary_height) = native::displays(mtm);
    let placement =
        placement::place_panel(&displays, native::cursor(primary_height), panel::PANEL_SIZE);
    let opened = gpui_kit::open_window(panel::options(placement), cx, |window, cx| {
        cx.new(|cx| panel::PanelView::new(route, hotkey_at, window, cx))
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
