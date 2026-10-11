//! Token theming shared by the four views (Phase 1 R14, D26: native macOS 27).
//!
//! Four things every view needs, kept in one place so the four views agree:
//!
//! - **The card and its colours.** The panel is Liquid Glass (`shell::panel`); each
//!   view paints its whole card with [`surface`], `color.windowBackground`, opaque, so
//!   text contrast does not depend on what is behind the panel. Primary text
//!   ([`primary_text`]) is `color.label` composited over that card.
//! - **The shared controls**, one constructor per role, so every screen's buttons and
//!   fields agree: [`primary_button`] (the default action, filled with the system
//!   accent: macOS's default button), [`push_button`] (a plain push button),
//!   [`destructive_button`] (a filled red button, for a confirmed destructive action),
//!   [`text_field`] and [`switch`], all at macOS's 24 pt control height and 13 pt
//!   text. A selected row's fill and text come from [`selection`].
//! - **Live appearance switching.** [`FollowAppearance`] observes the panel window's
//!   appearance and re-applies the matching theme when the system flips between light
//!   and dark, so an open view repaints in place. In automation builds a
//!   `set_appearance light|dark` override wins until `set_appearance system` drops it.
//!   (The accent follows the system through `theme::refresh_accent`.)
//! - **Where the colours are** ([`Samples`]): each view reports, in its view-state's
//!   `theme_samples`, a region of its card, a region around a run of primary text, a
//!   region inside its primary button's fill and (Settings) one around a checked
//!   switch, in window points (probes/README.md). The regions are measured from the
//!   real elements at prepaint (a zero-paint [`canvas`] laid over each), never
//!   hard-coded, so they follow the layout.

use std::sync::Mutex;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Sizable, Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::theme::tokens::{font_size, size, spacing};

/// The smallest side a reported region may have (the probe's `STATE_REGION_MIN_PT`,
/// ruling R-q): a real surface, text run or control, never a swatch.
pub const SAMPLE_MIN: f32 = 24.0;

/// Push buttons and text fields: macOS's regular control height.
pub const CONTROL_H: Pixels = size::CONTROL;

/// A push button's side padding around its 13 pt label.
pub const BUTTON_PAD_X: Pixels = spacing::S3;

/// The primary button's side padding. Wider than a push button's, so the
/// [`PRIMARY_INSET_X`] + [`SAMPLE_MIN`] pt beside the label are pure fill for the
/// theming probe (a D26 deviation: macOS pads a default button like any other).
pub const PRIMARY_PAD_X: Pixels = px(32.);

/// A primary sample starts this far in: past the 6 pt control corner.
const PRIMARY_INSET_X: f32 = 7.0;
/// A surface region stays this far inside its element, off the neighbours beside it.
/// Not above or below: a surface spacer is a header row at the 24 pt control height,
/// with card above and below it.
const SURFACE_INSET: f32 = 2.0;
/// A text run or a switch is its bounds, grown (about its centre) to at least this.
const GROWN_MIN: f32 = 28.0;

/// `color.windowBackground`: the opaque card every view paints itself on. The theme
/// carries it at alpha 0 (so gpui-component's window root does not cover the glass);
/// the card is its RGB at full opacity.
pub fn surface(cx: &App) -> Hsla {
    opaque(cx.theme().background)
}

/// `color.label` as it lands on [`surface`].
pub fn primary_text(cx: &App) -> Hsla {
    over(cx.theme().foreground, surface(cx))
}

/// A selected row's fill and text: the accent-derived macOS selection
/// (`selectedContentBackgroundColor` with white text, `theme::accent`).
pub fn selection(cx: &App) -> (Hsla, Hsla) {
    crate::theme::selection(cx)
}

/// `color` at full opacity.
pub fn opaque(color: Hsla) -> Hsla {
    Hsla { a: 1.0, ..color }
}

/// `top` alpha-composited over the opaque `bottom` (sRGB "over"), as an opaque colour.
pub fn over(top: Hsla, bottom: Hsla) -> Hsla {
    let (t, b) = (Rgba::from(top), Rgba::from(bottom));
    let mix = |t: f32, b: f32| t * top.a + b * (1.0 - top.a);
    Hsla::from(Rgba {
        r: mix(t.r, b.r),
        g: mix(t.g, b.g),
        b: mix(t.b, b.b),
        a: 1.0,
    })
}

/// The view's default action: filled with the system accent (macOS's default push
/// button), white or, for a light accent, black text. Sized so a [`Sample::Primary`]
/// region fits in its fill beside the label. Callers add `.label(..)`, `.on_click(..)`.
pub fn primary_button(id: impl Into<ElementId>) -> Button {
    Button::new(id).primary().h(CONTROL_H).px(PRIMARY_PAD_X)
}

/// A plain macOS push button: the control fill, label-coloured text, no stroke.
pub fn push_button(id: impl Into<ElementId>) -> Button {
    Button::new(id).secondary().h(CONTROL_H).px(BUTTON_PAD_X)
}

/// A confirmed destructive action (as an alert's destructive button): filled red,
/// white text.
pub fn destructive_button(id: impl Into<ElementId>) -> Button {
    Button::new(id).danger().h(CONTROL_H).px(BUTTON_PAD_X)
}

/// A single-line text field at the control height with 13 pt text.
pub fn text_field(state: &Entity<InputState>) -> Input {
    Input::new(state).h(CONTROL_H).text_size(font_size::ROW)
}

/// A switch at AppKit's small size (gpui-component `small`, 28 x 16): checked takes
/// the system accent (`primary`), off the AA-raised track.
pub fn switch(id: impl Into<ElementId>) -> Switch {
    Switch::new(id).small()
}

/// Which of a view's samples an element stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    /// An empty stretch of the opaque card: the region is its bounds, inset.
    Surface,
    /// A run of primary text: the region is its bounds, grown to at least 28 pt.
    Text,
    /// The primary button: the region is the fill beside its label, full height.
    Primary,
    /// A checked switch (the system accent): its bounds grown to at least 28 pt; the
    /// probe takes the best-matching pixel, since a 16 pt track holds no 24 pt square.
    Accent,
}

/// A region in window points from the panel's top-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// `state.view.theme_samples`: where the view's card, primary text, primary button
/// and (Settings) checked switch are, or `null` for one not drawn right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeSamples {
    pub surface: Option<Region>,
    pub text: Option<Region>,
    pub primary: Option<Region>,
    pub accent: Option<Region>,
}

/// `bounds` grown about its centre to at least [`GROWN_MIN`] each way.
fn grown(x: f32, y: f32, w: f32, h: f32) -> Region {
    let (gw, gh) = (w.max(GROWN_MIN), h.max(GROWN_MIN));
    Region {
        x: x - (gw - w) / 2.0,
        y: y - (gh - h) / 2.0,
        w: gw,
        h: gh,
    }
}

/// The region `which` reports for an element laid out at `bounds` (window points).
pub fn region_for(which: Sample, bounds: Bounds<Pixels>) -> Region {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    match which {
        Sample::Surface => Region {
            x: x + SURFACE_INSET,
            y,
            w: (w - 2.0 * SURFACE_INSET).max(0.0),
            h,
        },
        Sample::Text | Sample::Accent => grown(x, y, w, h),
        // The primary variant draws no stroke, so its fill runs edge to edge
        // vertically; horizontally the region starts past the corner.
        Sample::Primary => Region {
            x: x + PRIMARY_INSET_X,
            y,
            w: SAMPLE_MIN,
            h,
        },
    }
}

/// One view's sample regions. `state()` is context-free (the router's `TABLE` row), so
/// they live in a static the view's elements write at prepaint.
pub struct Samples(Mutex<ThemeSamples>);

impl Samples {
    pub const fn new() -> Self {
        Samples(Mutex::new(ThemeSamples {
            surface: None,
            text: None,
            primary: None,
            accent: None,
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ThemeSamples> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self) -> ThemeSamples {
        *self.lock()
    }

    /// Forget every region: called at the top of each render, so a sample whose element
    /// is gone this frame reads `null` instead of a stale place.
    pub fn clear(&self) {
        *self.lock() = ThemeSamples::default();
    }

    fn set(&self, which: Sample, region: Region) {
        let mut s = self.lock();
        let slot = match which {
            Sample::Surface => &mut s.surface,
            Sample::Text => &mut s.text,
            Sample::Primary => &mut s.primary,
            Sample::Accent => &mut s.accent,
        };
        *slot = Some(region);
    }

    /// A zero-paint element laid over its parent (which must be `relative()`) that
    /// records the parent's bounds as `which` at prepaint.
    pub fn probe(&'static self, which: Sample) -> impl IntoElement {
        canvas(
            move |bounds, _, _| self.set(which, region_for(which, bounds)),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// `element` with a [`Samples::probe`] laid over it.
    pub fn mark(&'static self, which: Sample, element: impl IntoElement) -> Div {
        div()
            .relative()
            .flex_shrink_0()
            .child(element)
            .child(self.probe(which))
    }
}

/// Keeps a view's theme on the system appearance while it is open (see the module doc).
#[derive(Default)]
pub struct FollowAppearance(Option<Subscription>);

impl FollowAppearance {
    /// Start following, once per view; call from `render`.
    pub fn ensure<V: 'static>(&mut self, window: &mut Window, cx: &mut Context<V>) {
        if self.0.is_some() {
            return;
        }
        self.0 =
            Some(cx.observe_window_appearance(window, |_, window, cx| follow_system(window, cx)));
        // Not inside render: changing the global theme there would race the frame.
        window.defer(cx, follow_system);
    }
}

/// Apply the token theme matching `window`'s appearance, unless an automation
/// `set_appearance light|dark` override holds.
fn follow_system(window: &mut Window, cx: &mut App) {
    #[cfg(feature = "automation")]
    if crate::automation::store::appearance() != crate::automation::protocol::Appearance::System {
        return;
    }
    let mode = ThemeMode::from(window.appearance());
    if cx.theme().mode != mode {
        Theme::change(mode, Some(window), cx);
        cx.refresh_windows();
    }
}

#[cfg(test)]
mod tests {
    use super::{over, region_for, Sample, CONTROL_H, PRIMARY_PAD_X, SAMPLE_MIN};
    use crate::theme::accent::{derive, Rgb, Source};
    use crate::theme::{dark_theme_config, light_theme_config};
    use gpui_kit::component::ThemeMode;
    use gpui_kit::{point, px, size, Bounds, Hsla, Rgba};

    fn hsla(hex: &str) -> Hsla {
        Hsla::from(Rgba::try_from(hex).unwrap_or_else(|e| panic!("{hex}: {e}")))
    }

    fn rgb8(c: Hsla) -> [u8; 3] {
        let c = Rgba::from(c);
        [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8)
    }

    fn close(got: [u8; 3], want: [u8; 3]) -> bool {
        got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1)
    }

    /// R14/D26 RED/GREEN anchor: the colours the views paint, derived from the
    /// converted themes through the same helpers the views call, are the ones the
    /// theming probe expects (probes/theming.json's literals, so the proof cannot move
    /// with the tokens): the opaque card, label over it, the primary button's fill and
    /// the checked switch (`primary`), in both modes, for the default blue accent.
    /// Also pins that each sample region a view can report is at least 24 pt.
    #[test]
    fn theme_matches_tokens() {
        let blue = Source::bare(Rgb::parse("#007aff").unwrap());
        for (name, config, surface, text) in [
            (
                "light",
                light_theme_config(&derive(&blue, ThemeMode::Light)),
                [0xff; 3],
                [0x26; 3],
            ),
            (
                "dark",
                dark_theme_config(&derive(&blue, ThemeMode::Dark)),
                [0x1e; 3],
                [0xdd; 3],
            ),
        ] {
            let c = &config.colors;
            let background = hsla(c.background.as_deref().expect("background"));
            assert_eq!(background.a, 0.0, "{name}: the window root stays clear");
            let background = super::opaque(background);
            assert!(
                close(rgb8(background), surface),
                "{name} surface {:?}",
                rgb8(background)
            );
            let foreground = hsla(c.foreground.as_deref().expect("foreground"));
            let text_on_card = over(foreground, background);
            assert!(
                close(rgb8(text_on_card), text),
                "{name} text {:?}",
                rgb8(text_on_card)
            );
            let button = hsla(c.button_primary.as_deref().expect("button_primary"));
            assert!(
                close(rgb8(button), [0x00, 0x72, 0xef]),
                "{name} primary {:?}",
                rgb8(button)
            );
            let switch = hsla(c.primary.as_deref().expect("primary"));
            assert!(
                close(rgb8(switch), [0x00, 0x7a, 0xff]),
                "{name} switch {:?}",
                rgb8(switch)
            );
        }

        // Regions: a 24 pt primary button yields a 24 x 24 region inside its fill and
        // left of the label; a short text run or a switch grows to 28; a surface is
        // inset.
        let at = |x: f32, y: f32, w: f32, h: f32| Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        };
        let primary = region_for(Sample::Primary, at(500., 10., 120., f32::from(CONTROL_H)));
        assert!(
            primary.w >= SAMPLE_MIN && primary.h >= SAMPLE_MIN,
            "{primary:?}"
        );
        assert!(
            primary.x + primary.w <= 500. + f32::from(PRIMARY_PAD_X),
            "clear of the label"
        );
        assert!(primary.x >= 500. + 6., "clear of the 6 pt corner");
        let text = region_for(Sample::Text, at(20., 40., 60., 18.));
        assert_eq!((text.w, text.h), (60., 28.));
        assert_eq!((text.x, text.y), (20., 35.));
        let switch = region_for(Sample::Accent, at(20., 40., 28., 16.));
        assert_eq!(
            (switch.x, switch.y, switch.w, switch.h),
            (20., 34., 28., 28.)
        );
        let surface = region_for(Sample::Surface, at(100., 8., 200., 24.));
        assert_eq!(
            (surface.x, surface.y, surface.w, surface.h),
            (102., 8., 196., 24.)
        );
    }
}
