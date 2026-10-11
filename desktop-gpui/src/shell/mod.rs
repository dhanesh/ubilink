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
    /// D26: AppKit reported a system colour change (the accent, `theme::accent`):
    /// rebuild the theme and repaint in place.
    AccentChanged,
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
    /// Where the panel is and how big (`None` when there were no displays to place it
    /// on). A route switch or a content change refits it from here.
    placement: Option<placement::Placement>,
    /// The panel's AppKit window number, so native calls act on this window only.
    number: Option<isize>,
}

/// How long the deactivation after a file dialog is given to settle before the panel
/// takes key status again (see `end_file_dialog`).
const DEACTIVATE_SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

/// Whether a file dialog the panel opened is up: at most one at a time (a second
/// press must not stack another), and closing it restores the panel exactly once.
/// It also remembers whether opening it activated the app, which closing must undo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialogGate {
    open: bool,
    activated: bool,
}

impl DialogGate {
    /// Open a dialog; `app_was_active` is whether Ubilink was already the active app
    /// (the panel is non-activating, so normally it was not). False (do nothing) when
    /// one is already up.
    pub fn begin(&mut self, app_was_active: bool) -> bool {
        if self.open {
            return false;
        }
        *self = DialogGate {
            open: true,
            activated: !app_was_active,
        };
        true
    }

    /// A dialog is up right now.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The dialog is gone (answered, cancelled, or its panel went away): what to undo,
    /// only for the first call; `None` after that.
    pub fn end(&mut self, panel_stays: bool) -> Option<DialogEnd> {
        if !self.open {
            return None;
        }
        let activated = self.activated;
        *self = DialogGate::default();
        Some(DialogEnd {
            deactivate: activated,
            refocus_panel: panel_stays,
        })
    }
}

/// What closing a file dialog undoes, to get back to the state before Import: the app
/// not active (the user's previous app frontmost, as the non-activating panel left
/// it) and, if the panel stays, the panel key at its pop-up level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogEnd {
    /// Opening the dialog activated Ubilink: deactivate it, so the previous app is
    /// frontmost again and gets the keystrokes once the panel goes.
    pub deactivate: bool,
    /// The panel is still open (Settings): back to its level, and key again.
    pub refocus_panel: bool,
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
    dialog: DialogGate,
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
    // D26: follow the system accent colour live.
    crate::theme::accent::observe(inbox.clone());

    let mut state = startup_state();

    let tray = match tray::build() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("[ubilink] tray item unavailable: {e}");
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
        dialog: DialogGate::default(),
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
    // Warm the glass view too, so the first real show does not load it.
    if let Some(n) = native::newest_panel_number(mtm) {
        native::apply_glass(mtm, n, panel::CORNER_RADIUS);
    }
    match opened {
        // The close runs as a foreground task, i.e. after this run-loop turn's Core
        // Animation commit, which is the expensive part being warmed.
        Ok((handle, _)) => cx
            .spawn(async move |cx| {
                let _ = cx.update(|cx| handle.update(cx, |_, window, _| window.remove_window()));
            })
            .detach(),
        Err(e) => eprintln!("[ubilink] panel prewarm failed: {e}"),
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
        eprintln!("[ubilink] shell state: {json}");
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
    if let ShellEvent::AccentChanged = event {
        crate::theme::refresh_accent(cx);
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
        ShellEvent::AccentChanged => None,
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
    let placement = placement::place_panel(
        &displays,
        native::cursor(primary_height),
        panel::size_for(route),
    );
    let opened = gpui_kit::open_window(panel::options(placement), cx, |window, cx| {
        cx.new(|cx| panel::PanelView::new(route, hotkey_at, window, cx))
    });
    let (handle, view) = match opened {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[ubilink] panel failed to open: {e}");
            return;
        }
    };
    // D26: Liquid Glass behind the content (macOS 26+; vibrancy before). Then read
    // back what AppKit actually built (the window is ordered front synchronously by
    // GPUI's open), so the state reports facts rather than the request.
    let number = native::newest_panel_number(mtm);
    if let Some(n) = number {
        native::apply_glass(mtm, n, panel::CORNER_RADIUS);
    }
    let facts = native::inspect_panel(mtm);
    cx.update_global::<Shell, _>(|shell, _| {
        shell.panel = Some(Panel {
            handle,
            view,
            route,
            placement,
            number,
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
    // A file dialog belongs to the view being replaced: close it and restore the panel.
    end_file_dialog(true, cx);
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
    // The same window takes the new route's height (the Tauri panel refit on every
    // view switch).
    fit_panel(views_height(route), cx);
}

fn views_height(route: Route) -> f64 {
    crate::views::height_for(route)
}

/// Resize the open panel to `height` points (clamped as `panel::clamp_height`) on the
/// display it is on: its top stays unless the taller panel would run off the bottom,
/// and it is cut to the display's usable area (`placement::fit_on`; the Tauri shell's
/// `resize_panel`). A route switch calls this, and so does a view whose content
/// changed size (Settings' reset question). No panel, or no change: nothing to do.
///
/// The frame is set from a foreground task, never inside this call: AppKit reports
/// the resize back to GPUI synchronously (windowDidResize), and GPUI handles that by
/// updating the app, which must not be borrowed at the time. GPUI's own
/// `Window::resize` defers for the same reason (and keeps the bottom edge, not the
/// top, which is why it is not used).
pub fn fit_panel(height: f64, cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some((handle, Some(current), Some(number))) = cx
        .try_global::<Shell>()
        .and_then(|s| s.panel.as_ref())
        .map(|p| (p.handle, p.placement, p.number))
    else {
        return;
    };
    let (displays, primary_height) = native::displays(mtm);
    let Some(next) = placement::refit(
        &displays,
        current.display_id,
        current.global,
        // The nominal width, not the current one: a narrow display may have cut it.
        (panel::PANEL_WIDTH, panel::window_height(height)),
    ) else {
        return;
    };
    if next == current {
        return;
    }
    cx.update_global::<Shell, _>(|shell, _| {
        if let Some(p) = shell.panel.as_mut().filter(|p| p.handle == handle) {
            p.placement = Some(next);
        }
    });
    cx.spawn(async move |cx| {
        // Still the same panel? (It may have been hidden meanwhile.)
        let same = cx.update(|cx| {
            cx.try_global::<Shell>()
                .and_then(|s| s.panel.as_ref())
                .is_some_and(|p| p.handle == handle)
        });
        if same {
            if let Some(mtm) = MainThreadMarker::new() {
                native::set_panel_frame(mtm, number, next.global, next.size, primary_height);
            }
        }
    })
    .detach();
}

fn hide(cx: &mut App) {
    end_file_dialog(false, cx);
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
        end_file_dialog(false, cx);
        mark_hidden(cx);
    }
}

/// Hide the panel only if `handle` is still the shell's panel: a dismiss asked for
/// by one window (Escape) must never hide a newer one opened since.
pub fn hide_if_current(handle: AnyWindowHandle, cx: &mut App) {
    let current = cx
        .try_global::<Shell>()
        .and_then(|s| s.panel.as_ref())
        .is_some_and(|p| p.handle == handle);
    if current {
        hide(cx);
    }
}

/// Settings' Import is about to open a file dialog. GPUI opens it with
/// `beginWithCompletionHandler` and neither activates the app nor sets a level, while
/// the panel is a non-activating NSPanel at the pop-up level (101) of an accessory
/// app: the dialog would open behind the panel, unseen. So activate the app and drop
/// the panel to the normal level until [`end_file_dialog`]. False (open nothing) when
/// a dialog is already up: presses never stack dialogs.
pub fn begin_file_dialog(cx: &mut App) -> bool {
    if cx
        .try_global::<Shell>()
        .and_then(|s| s.panel.as_ref())
        .is_none()
    {
        return false;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let was_active = native::app_is_active(mtm);
    if !cx.update_global::<Shell, _>(|shell, _| shell.dialog.begin(was_active)) {
        return false;
    }
    // Lower the panel so the dialog is not under it. Without the recorded window
    // number, fall back to the newest visible panel (file dialogs are skipped); if
    // there is none either, the dialog still opens, but may sit under the panel.
    let number = cx
        .try_global::<Shell>()
        .and_then(|s| s.panel.as_ref())
        .and_then(|p| p.number)
        .or_else(|| native::newest_panel_number(mtm));
    if let Some(number) = number {
        native::lower_panel_for_dialog(mtm, number);
    }
    cx.activate(true);
    true
}

/// The file dialog is over: it was answered (`panel_stays`), or the view that opened
/// it is going away (a route switch: the panel stays; a hide or a click-away close: it
/// does not). Close any dialog still up, deactivate the app if opening the dialog
/// activated it, and, when the panel stays, put it back at its level with key status.
/// Once, from a foreground task (outside this update: the dialog's completion handler
/// and the key change call back into AppKit and GPUI).
pub fn end_file_dialog(panel_stays: bool, cx: &mut App) {
    if cx.try_global::<Shell>().is_none() {
        return;
    }
    let Some(end) = cx.update_global::<Shell, _>(|shell, _| shell.dialog.end(panel_stays)) else {
        return;
    };
    let target = cx
        .try_global::<Shell>()
        .and_then(|s| s.panel.as_ref())
        .and_then(|p| p.number.map(|n| (p.handle, n)));
    cx.spawn(async move |cx| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        native::cancel_file_dialogs(mtm);
        // Deactivate first: the previous app becomes active again. A non-activating
        // panel can then take key status without activating Ubilink, which is the
        // state the panel was in before Import. App switching completes on a later
        // run-loop turn, so the panel takes key after a short wait, not before the
        // switch could take it away again.
        if end.deactivate {
            native::deactivate_app(mtm);
            if end.refocus_panel {
                cx.background_executor().timer(DEACTIVATE_SETTLE).await;
            }
        }
        let Some((handle, number)) = target.filter(|_| end.refocus_panel) else {
            return;
        };
        // Re-check after the wait: a new Import may have opened a dialog (the panel
        // must stay under it), or the panel may have been replaced or hidden.
        let refocus = cx.update(|cx| {
            cx.try_global::<Shell>().is_some_and(|s| {
                should_refocus(
                    s.dialog.is_open(),
                    s.panel.as_ref().map(|p| p.handle),
                    handle,
                )
            })
        });
        if refocus {
            native::restore_panel_after_dialog(mtm, number);
        }
    })
    .detach();
}

/// After a file dialog closed and the deactivation settled: give the panel key status
/// back only if no new dialog has opened since and the panel is still the one that
/// opened the dialog (same window handle).
pub fn should_refocus<H: PartialEq>(
    dialog_open_now: bool,
    current: Option<H>,
    ended_on: H,
) -> bool {
    !dialog_open_now && current.is_some_and(|h| h == ended_on)
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
    use super::{
        command_for, should_refocus, startup_state, Command, DialogEnd, DialogGate, Route,
        ShellEvent,
    };

    /// One file dialog at a time, and its end restores the panel exactly once (a
    /// route switch and the dialog's own answer may both report the end).
    #[test]
    fn the_dialog_gate_admits_one_dialog_and_ends_it_once() {
        let mut g = DialogGate::default();
        assert!(g.begin(false), "the first press opens a dialog");
        assert!(
            !g.begin(false),
            "a second press while it is up does not stack another"
        );
        assert!(g.end(true).is_some(), "the first end restores the panel");
        assert_eq!(
            g.end(true),
            None,
            "a later end (the cancelled answer) does nothing"
        );
        assert!(g.begin(false), "a new dialog may open afterwards");
    }

    /// Closing the dialog returns to the state before Import: Ubilink inactive (the
    /// previous app frontmost) and, if the panel stays, the panel key again.
    #[test]
    fn closing_the_dialog_undoes_the_activation_it_caused() {
        let end = |was_active, panel_stays| {
            let mut g = DialogGate::default();
            assert!(g.begin(was_active));
            g.end(panel_stays).unwrap()
        };
        // The usual case: the non-activating panel left Ubilink inactive.
        assert_eq!(
            end(false, true),
            DialogEnd {
                deactivate: true,
                refocus_panel: true
            },
            "answered, Settings stays: deactivate, then the panel is key again"
        );
        assert_eq!(
            end(false, false),
            DialogEnd {
                deactivate: true,
                refocus_panel: false
            },
            "hidden while the dialog was up: deactivate, nothing to refocus"
        );
        // Ubilink was already active before Import: leave it so.
        assert_eq!(
            end(true, true),
            DialogEnd {
                deactivate: false,
                refocus_panel: true
            }
        );
        assert_eq!(
            end(true, false),
            DialogEnd {
                deactivate: false,
                refocus_panel: false
            }
        );
    }

    /// The delayed refocus re-checks: Import pressed again within the settle window
    /// opens a new dialog, and the old refocus must not raise the panel over it; a
    /// replaced or hidden panel is not refocused either.
    #[test]
    fn a_stale_refocus_never_raises_the_panel_over_a_new_dialog() {
        let mut g = DialogGate::default();
        assert!(g.begin(false));
        let ended = g.end(true).unwrap();
        assert!(ended.refocus_panel);
        // Within the settle window, Import again.
        assert!(g.begin(false));
        assert!(
            !should_refocus(g.is_open(), Some(1u8), 1),
            "a new dialog is up"
        );
        // No new dialog: refocus only the same panel.
        assert!(should_refocus(false, Some(1u8), 1));
        assert!(
            !should_refocus(false, Some(2u8), 1),
            "replaced by another panel"
        );
        assert!(!should_refocus(false, None::<u8>, 1), "hidden");
    }

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
