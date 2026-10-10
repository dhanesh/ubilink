//! Token theming shared by the four views (Phase 1 R14).
//!
//! Three things every view needs, kept in one place so the four views agree:
//!
//! - **The colours.** The panel window is an NSVisualEffectView blur and `color.uiBg` is
//!   translucent, so on its own a view's background is whatever is behind the panel.
//!   Each view paints its whole card with [`surface`]: `color.uiBg`'s RGB at full
//!   opacity. Primary text ([`primary_text`]) is `color.text1` alpha-composited over
//!   that surface, so the colour on screen is the token's, not the desktop's.
//!   The accent control is a primary `Button` ([`accent_button`]), filled with
//!   `color.accent` by gpui-component (`primary` and `button_primary` both come from it,
//!   `theme/mod.rs`).
//! - **Live appearance switching.** [`FollowAppearance`] observes the panel window's
//!   appearance and re-applies the matching token theme when the system flips between
//!   light and dark, so an open view repaints in place: no new window, no restart. It
//!   also catches up once when a view is first drawn, for a change made while the
//!   panel was hidden. In automation builds a `set_appearance light|dark` override
//!   wins over the system until `set_appearance system` drops it.
//! - **Where the colours are** ([`Samples`]): each view reports, in its view-state's
//!   `theme_samples`, a region of its opaque surface, a region around a run of primary
//!   text and a region inside its accent control, in window points
//!   (probes/README.md, "Phase 1 view-state contract"). The regions are measured from
//!   the real elements at prepaint (a zero-paint [`canvas`] laid over each), never
//!   hard-coded, so they follow the layout.

use std::sync::Mutex;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable, Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

/// The smallest side a reported region may have (the probe's `STATE_REGION_MIN_PT`,
/// ruling R-q): a real surface, text run or control, never a swatch.
pub const SAMPLE_MIN: f32 = 24.0;

/// Height of an accent button. With the control radius (8 pt) a 24 pt tall region
/// fits inside it 2 pt from the top and bottom and 6 pt from the side, clear of the
/// rounded corners' antialiasing.
pub const ACCENT_H: Pixels = px(28.);

/// Horizontal padding of an accent button: the label starts this far in, so the
/// [`ACCENT_INSET_X`] + [`SAMPLE_MIN`] pt beside it are pure accent fill with a margin
/// before the first glyph.
pub const ACCENT_PAD_X: Pixels = px(34.);

const ACCENT_INSET_X: f32 = 6.0;
const ACCENT_INSET_Y: f32 = 2.0;
/// A surface region stays this far inside its element, off any neighbour's edge.
const SURFACE_INSET: f32 = 2.0;
/// A text region is the run's bounds, grown (about its centre) to at least this.
const TEXT_MIN: f32 = 28.0;

/// `color.uiBg`'s RGB at full opacity: the card every view paints itself on.
pub fn surface(cx: &App) -> Hsla {
    opaque(cx.theme().background)
}

/// `color.text1` as it lands on [`surface`]: composited once, here, so primary text
/// has the token's colour on screen whatever the window is blurred over.
pub fn primary_text(cx: &App) -> Hsla {
    over(cx.theme().foreground, surface(cx))
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

/// A primary (accent-filled) button, sized so a [`Sample::Accent`] region fits in its
/// fill beside the label. Callers add `.label(..)`, `.on_click(..)` and so on.
pub fn accent_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .primary()
        .small()
        .h(ACCENT_H)
        .px(ACCENT_PAD_X)
}

/// Which of a view's three samples an element stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    /// An empty stretch of the opaque surface: the region is its bounds, inset.
    Surface,
    /// A run of primary text: the region is its bounds, grown to at least 28 pt.
    Text,
    /// An accent button: the region is the fill beside its label.
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

/// `state.view.theme_samples`: where the view's surface, primary text and accent
/// control are, or `null` for one the view is not drawing right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeSamples {
    pub surface: Option<Region>,
    pub text: Option<Region>,
    pub accent: Option<Region>,
}

/// The region `which` reports for an element laid out at `bounds` (window points).
pub fn region_for(which: Sample, bounds: Bounds<Pixels>) -> Region {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    match which {
        Sample::Surface => Region {
            x: x + SURFACE_INSET,
            y: y + SURFACE_INSET,
            w: (w - 2.0 * SURFACE_INSET).max(0.0),
            h: (h - 2.0 * SURFACE_INSET).max(0.0),
        },
        Sample::Text => {
            let (gw, gh) = (w.max(TEXT_MIN), h.max(TEXT_MIN));
            Region {
                x: x - (gw - w) / 2.0,
                y: y - (gh - h) / 2.0,
                w: gw,
                h: gh,
            }
        }
        Sample::Accent => Region {
            x: x + ACCENT_INSET_X,
            y: y + ACCENT_INSET_Y,
            w: SAMPLE_MIN,
            h: (h - 2.0 * ACCENT_INSET_Y).max(0.0),
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
    use super::{over, region_for, Sample, ACCENT_H, ACCENT_PAD_X, SAMPLE_MIN};
    use crate::theme::{dark_theme_config, light_theme_config};
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

    /// R14 RED/GREEN anchor: the colours the views paint, derived from the converted
    /// token themes through the same helpers the views call, are the ones the theming
    /// probe expects (probes/theming.json's notes, written there as literals so the
    /// proof cannot move with the tokens): an opaque `uiBg` surface, `text1` composited
    /// over it, and `accent`, in both modes. Also pins that each sample region a view
    /// can report is at least the probe's 24 pt.
    #[test]
    fn theme_matches_tokens() {
        for (name, config, surface, text, accent) in [
            (
                "light",
                light_theme_config(),
                [0xfa, 0xfa, 0xfc],
                [0x1e; 3],
                [0x00, 0x7a, 0xff],
            ),
            (
                "dark",
                dark_theme_config(),
                [0x1c, 0x1c, 0x1e],
                [0xf6; 3],
                [0x0a, 0x84, 0xff],
            ),
        ] {
            let c = &config.colors;
            let background = hsla(c.background.as_deref().expect("background"));
            let foreground = hsla(c.foreground.as_deref().expect("foreground"));
            assert!(
                background.a < 1.0,
                "{name}: uiBg is translucent in the tokens"
            );
            let painted = super::opaque(background);
            assert_eq!(painted.a, 1.0);
            assert!(
                close(rgb8(painted), surface),
                "{name} surface {:?}",
                rgb8(painted)
            );
            let text_on_surface = over(foreground, painted);
            assert_eq!(text_on_surface.a, 1.0);
            assert!(
                close(rgb8(text_on_surface), text),
                "{name} text {:?}",
                rgb8(text_on_surface)
            );
            for fill in [c.primary.as_deref(), c.accent.as_deref()] {
                let fill = hsla(fill.expect("accent"));
                assert_eq!(fill.a, 1.0, "{name}: accent is opaque");
                assert!(close(rgb8(fill), accent), "{name} accent {:?}", rgb8(fill));
            }
        }

        // Regions: a 28 pt accent button yields a 24 x 24 region inside its fill and
        // left of the label; a short text run grows to 28; a surface is inset.
        let at = |x: f32, y: f32, w: f32, h: f32| Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        };
        let accent = region_for(Sample::Accent, at(500., 10., 120., f32::from(ACCENT_H)));
        assert!(
            accent.w >= SAMPLE_MIN && accent.h >= SAMPLE_MIN,
            "{accent:?}"
        );
        assert!(
            accent.x + accent.w < 500. + f32::from(ACCENT_PAD_X),
            "clear of the label"
        );
        assert!(accent.y > 10. && accent.y + accent.h < 10. + f32::from(ACCENT_H));
        let text = region_for(Sample::Text, at(20., 40., 60., 18.));
        assert_eq!((text.w, text.h), (60., 28.));
        assert_eq!((text.x, text.y), (20., 35.));
        let surface = region_for(Sample::Surface, at(100., 8., 200., 28.));
        assert_eq!(
            (surface.x, surface.y, surface.w, surface.h),
            (102., 10., 196., 24.)
        );
    }
}
