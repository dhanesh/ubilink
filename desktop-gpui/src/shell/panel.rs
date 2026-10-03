//! The pop-up panel (R4): one `WindowKind::PopUp` window — on macOS an NSPanel with the
//! non-activating style at NSPopUpWindowLevel, so opening it does not make ubilink the
//! frontmost app and capture still sees the app the user was in (the Tauri shell's
//! "render-first" reason for accessory + overlay) — over a `Blurred` background (a real
//! NSVisualEffectView, the Tauri shell's HUD vibrancy).
//!
//! The view is a placeholder named after the route; Task 13's router replaces it.
//!
//! Hiding closes the window (`remove_window`) and showing opens a fresh one at the
//! cursor's display: GPUI 0.3.7 has no per-window hide/show or move API, and reaching
//! behind it to `orderOut:` the NSWindow would leave GPUI's window state out of step.
//! The cost is a window build per show; R5's latency probe (Task 7) measures whether
//! that fits p50 < 80 ms, and reusing the window is the fallback if it does not.

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use super::placement::Placement;
use super::Route;
use crate::theme::tokens::{font_size, radius, spacing};

/// Panel size in points: the Tauri panel's (desktop/tauri.conf.json, 640x140).
pub const PANEL_SIZE: (f64, f64) = (640.0, 140.0);

/// The window kind and background the shell asks GPUI for. `ShellState` reports these
/// names until a live panel has been read back (see `native::inspect_panel`).
pub const REQUESTED_KIND: &str = "popup";
pub const REQUESTED_BACKGROUND: &str = "blurred";

pub fn options(placement: Option<Placement>) -> WindowOptions {
    let (x, y) = placement.map_or((0.0, 0.0), |p| p.local);
    let (w, h) = PANEL_SIZE;
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

pub struct PanelView {
    route: Route,
    /// Only a true→false transition hides: the panel can report inactive before it
    /// first becomes key, and that must not close it.
    was_active: bool,
    _activation: Subscription,
}

impl PanelView {
    pub fn new(route: Route, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Click-away (R5, proven in Task 5): capture and browser get out of the way when
        // the panel loses key status; settings and onboarding stay open.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if this.was_active && !active && this.route.hides_on_deactivate() {
                let id = window.window_handle().window_id();
                window.remove_window();
                super::panel_closed(id, cx);
            }
            this.was_active = active;
        });
        PanelView {
            route,
            was_active: window.is_window_active(),
            _activation: activation,
        }
    }

    pub fn set_route(&mut self, route: Route, cx: &mut Context<Self>) {
        self.route = route;
        cx.notify();
    }
}

impl Render for PanelView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(spacing::S1)
            .p(spacing::S5)
            .rounded(radius::CARD)
            // Translucent so the NSVisualEffectView's blur shows through.
            .bg(theme.background.opacity(0.55))
            .child(
                div()
                    .text_size(font_size::TITLE)
                    .text_color(theme.foreground)
                    .child(self.route.title()),
            )
            .child(
                div()
                    .text_size(font_size::META)
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "route: {} — placeholder until the router lands (Task 13)",
                        self.route.as_str()
                    )),
            )
    }
}
