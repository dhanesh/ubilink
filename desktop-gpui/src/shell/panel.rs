//! The pop-up panel (R4): one `WindowKind::PopUp` window — on macOS an NSPanel with the
//! non-activating style at NSPopUpWindowLevel, so opening it does not make ubilink the
//! frontmost app and capture still sees the app the user was in (the Tauri shell's
//! "render-first" reason for accessory + overlay) — over Liquid Glass (D26: an
//! NSGlassEffectView on macOS 26+, `native::apply_glass`; GPUI's `Blurred`
//! NSVisualEffectView before).
//!
//! The glass shows as a [`FRAME`]-wide rim around the content, like a macOS 27
//! popover: each view paints an opaque card (`theming::surface`) inset by the frame,
//! with corners concentric to the panel's ([`CORNER_RADIUS`] − frame). The frame is
//! added here, in the window's size, so a view's height is its card's.
//!
//! The view hosted inside is built by Task 13's router (`crate::views::build_for`):
//! `PanelView` itself only owns the glass frame and the type-erased child view,
//! and asks `Route::hides_on_deactivate` (which reads the router table) for the
//! click-away rule below, rather than deciding it itself.
//!
//! Hiding closes the window (`remove_window`) and showing opens a fresh one at the
//! cursor's display: GPUI 0.3.7 has no per-window hide/show or move API, and reaching
//! behind it to `orderOut:` the NSWindow would leave GPUI's window state out of step.
//! The cost is a window build per show; R5's latency probe (`probes/panel-hotkey.json`)
//! measures it against U1 (p99 < 200 ms, p50 < 80 ms over 50 triggers). Steady-state
//! shows take ~15–30 ms in a debug build; only the process's first one was slow, which
//! `shell::prewarm` (with `prewarm_options` below) moves to launch.

use gpui_kit::*;

use super::placement::{Display, Placement};
use super::Route;
use crate::theme::tokens::{radius, size};
use crate::views;

/// Panel width in points: the Tauri panel's (its tauri.conf.json, 640; deleted in D24).
pub const PANEL_WIDTH: f64 = 640.0;

/// The height capture and onboarding open at: the Tauri window's configured 640x140.
/// Each route's height comes from the router (`views::height_for`); Settings fits its
/// content (`views::settings::panel_height`) and the browser its five two-line rows
/// (`views::browser::panel_height`, D26 amendment).
pub const PANEL_HEIGHT: f64 = 140.0;

/// The glass rim around the card, each side, in points.
pub const FRAME: f64 = 4.0;

/// The glass panel's corner radius (macOS 27 popover); the card's is this minus
/// [`FRAME`] (`tokens::radius`).
pub const CORNER_RADIUS: f64 = 18.0;

/// The card's width: what a view lays out across.
pub const CARD_WIDTH: f64 = PANEL_WIDTH - 2.0 * FRAME;

/// The window height for a view whose card is `card_height` tall, clamped.
pub fn window_height(card_height: f64) -> f64 {
    clamp_height(card_height + 2.0 * FRAME)
}

/// The Tauri shell's `resize_panel` clamp: no route is shorter or taller than this
/// (a display's usable area can cut it further, `placement::fit_on`).
pub const MIN_HEIGHT: f64 = 96.0;
pub const MAX_HEIGHT: f64 = 900.0;

/// `route`'s panel size: the panel width and the route's height, clamped.
pub fn size_for(route: Route) -> (f64, f64) {
    (PANEL_WIDTH, window_height(views::height_for(route)))
}

/// A wanted height clamped to [`MIN_HEIGHT`]..[`MAX_HEIGHT`].
pub fn clamp_height(h: f64) -> f64 {
    if h.is_finite() {
        h.clamp(MIN_HEIGHT, MAX_HEIGHT)
    } else {
        PANEL_HEIGHT
    }
}

pub fn options(placement: Option<Placement>) -> WindowOptions {
    let (x, y) = placement.map_or((0.0, 0.0), |p| p.local);
    let (w, h) = placement.map_or((PANEL_WIDTH, PANEL_HEIGHT), |p| p.size);
    WindowOptions {
        // Origin is relative to `display_id`'s frame (see placement.rs).
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(x as f32), px(y as f32)),
            size: size(px(w as f32), px(h as f32)),
        })),
        // No titlebar: GPUI then uses a full-size content view with the title hidden.
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: placement.map(|p| DisplayId::new(u64::from(p.display_id))),
        window_background: WindowBackgroundAppearance::Blurred,
        ..WindowOptions::default()
    }
}

/// The launch prewarm's panel (see `shell::prewarm`): the real panel's options, not
/// key, and placed a panel-width plus a margin left of the leftmost display, so it
/// is on no screen. The origin is relative to the primary display, whose global x is
/// 0, so the leftmost global x is directly usable.
pub fn prewarm_options(displays: &[Display]) -> WindowOptions {
    let w = PANEL_WIDTH;
    let left = displays.iter().map(|d| d.frame.x).fold(0.0, f64::min);
    let mut o = options(None);
    o.focus = false;
    if let Some(WindowBounds::Windowed(b)) = o.window_bounds.as_mut() {
        b.origin = point(px((left - w - 1000.0) as f32), px(0.0));
    }
    o
}

pub struct PanelView {
    route: Route,
    /// The current route's view, built by the router (`views::build_for`) and swapped
    /// on every `set_route`. Type-erased so this struct doesn't grow a variant per
    /// route as Phase 1 adds real views.
    child: AnyView,
    /// Only a true→false transition hides: the panel can report inactive before it
    /// first becomes key, and that must not close it.
    was_active: bool,
    _activation: Subscription,
    /// R5 timing (automation builds): the hotkey's t0, until the first frame is timed.
    #[cfg(feature = "automation")]
    first_frame: FirstFrame,
}

/// Where the hotkey-to-first-frame sample stands (U1/R5). See `PanelView::new`.
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, PartialEq)]
enum FirstFrame {
    /// No hotkey asked for this show (open_view, tray, reopen) or already recorded.
    Untimed,
    /// Waiting for the first platform frame; t0 attached.
    Pending(std::time::Instant),
    /// The first platform frame has started: the render inside it records the sample
    /// once that frame has been presented.
    Armed(std::time::Instant),
}

impl PanelView {
    pub fn new(
        route: Route,
        hotkey_at: Option<std::time::Instant>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let child = views::build_for(route, cx);
        Self::with_child(route, child, hotkey_at, window, cx)
    }

    /// The launch prewarm's panel (`shell::prewarm`): the real card and window, but a
    /// neutral child instead of a route's view, so warming never runs a view's
    /// constructor. Once Phase 1's views do real work (capture reads the frontmost
    /// app, browser opens the graph), building one at launch would be a side effect
    /// (O4). The child still draws a line of text in the theme font, so the text
    /// system's first-use cost is paid here too, not by the user's first show.
    pub fn prewarm(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let child = cx.new(|_| PrewarmChild).into();
        // The route only feeds the click-away rule, which never fires: this window is
        // never key (`prewarm_options` sets `focus: false`), and `panel_closed`
        // ignores a window that is not the shell's panel anyway.
        Self::with_child(Route::Capture, child, None, window, cx)
    }

    fn with_child(
        route: Route,
        child: AnyView,
        hotkey_at: Option<std::time::Instant>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Click-away (R5, proven in Task 5): capture and browser get out of the way when
        // the panel loses key status; settings and onboarding stay open. The rule itself
        // now lives in the router table (views::TABLE), not here.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            this.activation_changed(active, window, cx);
        });
        #[cfg(not(feature = "automation"))]
        let _ = hotkey_at;
        #[cfg(feature = "automation")]
        let first_frame = match hotkey_at {
            Some(t0) => {
                Self::arm_first_frame(window, cx);
                FirstFrame::Pending(t0)
            }
            None => FirstFrame::Untimed,
        };
        PanelView {
            route,
            child,
            was_active: window.is_window_active(),
            _activation: activation,
            #[cfg(feature = "automation")]
            first_frame,
        }
    }

    /// U1/R5: when is the panel's first frame on screen? gpui-pre 0.3.7 exposes no
    /// present or Metal-completion callback, so this takes the latest point it does
    /// expose that is provably after the present:
    ///
    /// - `open_window` draws the first scene synchronously but does not present it;
    ///   the present happens in the window's next platform frame (`on_request_frame`:
    ///   next-frame callbacks, then `draw`, then `present`, i.e. Metal
    ///   `present_drawable` + `commit`, all inside one `App::update`).
    /// - `on_next_frame` runs at the start of that platform frame, before its present.
    ///   It arms the sample and notifies, so the same frame draws again.
    /// - That render `defer`s the record. Deferred effects flush when the frame's
    ///   outermost `App::update` finishes, which is after `present` returned.
    ///
    /// So t1 is after the drawable was handed to Core Animation (the GPU and the
    /// compositor take it from there; that tail is not observable in-process). The
    /// forced redraw makes t1 slightly late, never early. A second `on_next_frame`
    /// would also be after the present, but up to a whole vsync later.
    #[cfg(feature = "automation")]
    fn arm_first_frame(window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            let _ = this.update(cx, |v, cx| {
                if let FirstFrame::Pending(t0) = v.first_frame {
                    v.first_frame = FirstFrame::Armed(t0);
                    cx.notify();
                }
            });
        });
    }

    /// The click-away rule, run on every key-status change. Returns whether it closed
    /// the panel.
    fn activation_changed(
        &mut self,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let close = self.was_active && !active && self.route.hides_on_deactivate();
        if close {
            let id = window.window_handle().window_id();
            window.remove_window();
            super::panel_closed(id, cx);
        }
        self.was_active = active;
        close
    }

    /// Automation `resign_key` (R6): the panel loses key status, through the same
    /// `activation_changed` the window-activation observer calls — no AppKit focus
    /// change, so it works headless of the user's focus and needs no Accessibility
    /// grant. It simulates a key → not-key transition, so the panel counts as having
    /// been key first (the guard that ignores an inactive report before the first key
    /// status is for real AppKit ordering, not for this).
    #[cfg(feature = "automation")]
    pub fn simulate_resign_key(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.was_active = true;
        self.activation_changed(false, window, cx)
    }

    pub fn set_route(&mut self, route: Route, cx: &mut Context<Self>) {
        self.route = route;
        self.child = views::build_for(route, cx);
        cx.notify();
    }
}

/// `PanelView::prewarm`'s child: static text, no state, no I/O.
struct PrewarmChild;

impl Render for PrewarmChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("ubilink")
    }
}

impl Render for PanelView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "automation")]
        if let FirstFrame::Armed(t0) = self.first_frame {
            self.first_frame = FirstFrame::Untimed;
            let route = self.route.as_str().to_string();
            // Runs after this frame's present (see `arm_first_frame`).
            cx.defer(move |_| {
                use crate::automation::store::{record_metric, MetricKind, Sample};
                record_metric(Sample {
                    kind: MetricKind::HotkeyToFirstFrame,
                    route,
                    ms: t0.elapsed().as_secs_f64() * 1000.0,
                });
            });
        }
        // Transparent: the glass behind shows in the frame around the view's card.
        #[cfg(not(feature = "automation"))]
        let _ = cx;
        div()
            .size_full()
            .p(size::FRAME)
            .rounded(radius::PANEL)
            .child(self.child.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::placement::{Display, Rect};
    use super::{
        clamp_height, prewarm_options, window_height, CARD_WIDTH, CORNER_RADIUS, FRAME, MAX_HEIGHT,
        MIN_HEIGHT, PANEL_HEIGHT, PANEL_WIDTH,
    };
    use crate::theme::tokens::{radius, size};
    use gpui_kit::px;

    #[test]
    fn the_glass_frame_matches_the_tokens_and_wraps_the_card() {
        assert_eq!(px(FRAME as f32), size::FRAME);
        assert_eq!(px(CORNER_RADIUS as f32), radius::PANEL);
        assert_eq!(CARD_WIDTH, 632.0);
        assert_eq!(window_height(140.0), 148.0);
        assert_eq!(window_height(5000.0), MAX_HEIGHT);
    }
    use gpui_kit::WindowBounds;

    fn display(id: u32, x: f64, y: f64, w: f64, h: f64) -> Display {
        let frame = Rect {
            x,
            y,
            width: w,
            height: h,
        };
        Display {
            id,
            frame,
            work: frame,
        }
    }

    #[test]
    fn prewarm_panel_is_on_no_display_and_never_key() {
        // A primary, a display to its left and one above: the panel must miss all.
        let displays = [
            display(1, 0.0, 0.0, 1440.0, 900.0),
            display(2, -1920.0, 0.0, 1920.0, 1080.0),
            display(3, 0.0, -1080.0, 1920.0, 1080.0),
        ];
        let o = prewarm_options(&displays);
        assert!(
            !o.focus,
            "the prewarm must not take key status from the user's app"
        );
        let Some(WindowBounds::Windowed(b)) = o.window_bounds else {
            panic!("windowed bounds expected");
        };
        let x = f64::from(f32::from(b.origin.x));
        let right = x + PANEL_WIDTH;
        for d in &displays {
            assert!(
                right < d.frame.x,
                "panel [{x}, {right}) overlaps display {}",
                d.id
            );
        }
    }

    #[test]
    fn heights_are_clamped_as_the_tauri_shell_clamped_them() {
        assert_eq!(clamp_height(140.0), 140.0);
        assert_eq!(clamp_height(20.0), MIN_HEIGHT);
        assert_eq!(clamp_height(5000.0), MAX_HEIGHT);
        assert_eq!(clamp_height(f64::NAN), PANEL_HEIGHT);
        assert_eq!((MIN_HEIGHT, MAX_HEIGHT), (96.0, 900.0));
    }
}
