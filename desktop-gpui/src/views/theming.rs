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
//!   accent: macOS's default button; [`primary_capsule_button`] is its capsule, for
//!   the link browser's footer), [`push_button`] (a plain push button),
//!   [`destructive_button`] (a filled red button, for a confirmed destructive action),
//!   [`text_field`] and [`switch`], all at macOS's 24 pt control height and 13 pt
//!   text. A selected row is [`two_line_row`]'s grey pill with its 3:1 edge
//!   ([`row_selection`]); a footer is [`footer_bar`] with capsule buttons.
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
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::theme::tokens::{font_size, radius, size, spacing, MONO_FONT_FAMILY};

/// The smallest side a reported region may have (the probe's `STATE_REGION_MIN_PT`,
/// ruling R-q): a real surface, text run or control, never a swatch.
pub const SAMPLE_MIN: f32 = 24.0;

/// Push buttons and text fields: macOS's regular control height.
pub const CONTROL_H: Pixels = size::CONTROL;

/// A push button's side padding around its 13 pt label.
pub const BUTTON_PAD_X: Pixels = spacing::S3;

/// The primary button's side padding (D26 deviation: macOS pads a default button like
/// any other). Every view now uses the capsule ([`primary_capsule_button`]), which
/// widens it to [`CAPSULE_PAD_X`] for the theming probe's sample.
pub const PRIMARY_PAD_X: Pixels = px(32.);

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

/// A selected row's pill (D26 amendment, Things-style; the browser and capture): the
/// subtle grey fill (`color.rowSelected`) and the 1 pt edge that gives the selection
/// 3:1 against the card (`color.controlBorder`, the theme's `input`). The row's text
/// keeps its own colours.
pub fn row_selection(cx: &App) -> (Hsla, Hsla) {
    (crate::theme::row_selected(cx), cx.theme().input)
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
/// button), white or, for a light accent, black text. Views use its capsule,
/// [`primary_capsule_button`]. Callers add `.label(..)`, `.on_click(..)`.
pub fn primary_button(id: impl Into<ElementId>) -> Button {
    Button::new(id).primary().h(CONTROL_H).px(PRIMARY_PAD_X)
}

/// A capsule primary button's side padding: past the 12 pt round end, a
/// [`SAMPLE_MIN`] pt [`Sample::PrimaryCapsule`] region still fits before the label.
pub const CAPSULE_PAD_X: Pixels = spacing::S8;

/// The default action as a capsule (`radius.pill`), for the link browser's footer
/// (D26 amendment, Things-style). Otherwise [`primary_button`].
pub fn primary_capsule_button(id: impl Into<ElementId>) -> Button {
    primary_button(id).rounded(radius::PILL).px(CAPSULE_PAD_X)
}

/// A plain macOS push button: the control fill, label-coloured text, no stroke.
pub fn push_button(id: impl Into<ElementId>) -> Button {
    Button::new(id).secondary().h(CONTROL_H).px(BUTTON_PAD_X)
}

/// A push button as a capsule (`radius.pill`), beside a capsule primary button in a
/// Things-style footer (D26 amendment). Otherwise [`push_button`].
pub fn push_capsule_button(id: impl Into<ElementId>) -> Button {
    push_button(id).rounded(radius::PILL)
}

/// A confirmed destructive action as a capsule (a Notes-style form's buttons are all
/// capsules). Otherwise [`destructive_button`].
pub fn destructive_capsule_button(id: impl Into<ElementId>) -> Button {
    destructive_button(id).rounded(radius::PILL)
}

/// Between a form row's label column and its control column.
pub const FORM_GAP: Pixels = spacing::S2;

/// A Notes-style form label (rc.6, Settings): the label in primary text at the row
/// size, followed by a colon. The colon is a plain glyph, outside the `text!` Label,
/// so the accessible name stays the bare word (`Permissions`). The element is
/// `relative`, so a caller may lay a measuring marker over it.
pub fn form_label(id: &'static str, text: &'static str, cx: &App) -> Div {
    div()
        .relative()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .text_size(font_size::ROW)
        .text_color(primary_text(cx))
        .child(text!(id = id, text))
        .child(":")
}

/// A Notes-style form row (rc.6, Settings): `label` ends on the label column's right
/// edge (`size::FORM_LABEL`), vertically centred on the control's first line
/// (`first_line_h` tall); the control column starts on one left edge for every row,
/// and holds `control` (its descriptions go under it, in the same column).
pub fn form_row(label: Div, first_line_h: Pixels, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(FORM_GAP)
        .w_full()
        .min_w_0()
        .child(
            div()
                .w(size::FORM_LABEL)
                .flex_shrink_0()
                .h(first_line_h)
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .child(label),
        )
        .child(div().flex_1().min_w_0().flex().flex_col().child(control))
}

/// A Things-style row's 1 pt edge (drawn only while selected, reserved always, so the
/// text never moves).
pub const ROW_EDGE: Pixels = px(1.);

/// How far a Things-style row's text sits in from its pill's outer edge.
pub const ROW_TEXT_INSET: Pixels = spacing::S2;

/// A two-line row's title line box (13 pt text) and URL line box (12 pt): with the
/// edge they leave 3 pt above and below inside `size::ROW_TWO_LINE`.
pub const ROW_TITLE_LINE_H: Pixels = px(17.);
pub const ROW_URI_LINE_H: Pixels = px(15.);

/// A Things-style two-line row (D26 amendment), shared by the link browser and the
/// capture view's backlinks: `title` (row size, primary text) over `uri` (meta size,
/// SF Mono, secondary text), both from one left edge and cut with an ellipsis. Selected,
/// it is a soft grey pill with a 3:1 edge ([`row_selection`]); its text keeps its
/// colours. The caller wraps it in its row element (ListItem or a focusable div) and may
/// add children (it is `relative`).
pub fn two_line_row(
    selected: bool,
    title: impl IntoElement,
    uri: impl IntoElement,
    cx: &App,
) -> Div {
    let (fill, edge) = row_selection(cx);
    div()
        .relative()
        .flex()
        .flex_col()
        .justify_center()
        .h(size::ROW_TWO_LINE)
        .px(ROW_TEXT_INSET - ROW_EDGE)
        .w_full()
        .min_w_0()
        .rounded(radius::ROW)
        .border(ROW_EDGE)
        .border_color(if selected { edge } else { transparent_black() })
        .when(selected, |d| d.bg(fill))
        .child(
            div()
                .w_full()
                .text_size(font_size::ROW)
                .line_height(ROW_TITLE_LINE_H)
                .text_color(primary_text(cx))
                .truncate()
                .child(title),
        )
        .child(
            div()
                .w_full()
                .text_size(font_size::META)
                .line_height(ROW_URI_LINE_H)
                .font_family(MONO_FONT_FAMILY)
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(uri),
        )
}

/// The hairline above a footer bar (`color.separator`, decorative, D26).
pub const HAIRLINE: Pixels = px(1.);

/// A footer bar's padding above and below its control-height row.
pub const FOOTER_PAD_Y: Pixels = spacing::S2;

/// A footer bar's height: the hairline, then one control-height row, padded.
pub fn footer_h() -> Pixels {
    HAIRLINE + FOOTER_PAD_Y * 2. + size::CONTROL
}

/// What a footer bar sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterSurface {
    /// The card itself, like the toolbar under a Things list.
    Card,
    /// The grouped surface (`color.underPageBackground`, gpui-component's `muted`),
    /// like the bar of Things' Quick Entry; its bottom corners follow the card's.
    Grouped,
}

/// A Things-style footer bar (D26 amendment), shared by the link browser and the
/// capture view: a hairline above it, then a row with the context at the left (the
/// caller's first child, `flex_1`) and the actions at the right, capsules with the one
/// accent action last. `inset_x` is the view's side inset, so the footer's content
/// lines up with the body's.
pub fn footer_bar(inset_x: Pixels, surface: FooterSurface, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .h(footer_h())
        .border_t(HAIRLINE)
        .border_color(cx.theme().border)
        .px(inset_x)
        .py(FOOTER_PAD_Y)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::S2)
        .when(surface == FooterSurface::Grouped, |d| {
            d.bg(cx.theme().muted).rounded_b(radius::CARD)
        })
}

/// A confirmed destructive action (as an alert's destructive button): filled red,
/// white text.
pub fn destructive_button(id: impl Into<ElementId>) -> Button {
    Button::new(id).danger().h(CONTROL_H).px(BUTTON_PAD_X)
}

/// A text field's border: gpui-component's `Input` draws a 1 pt `border_1`.
pub const TEXT_FIELD_BORDER: Pixels = px(1.);

/// A single-line text field's vertical padding. The field's 13 pt text lays out in a
/// 1.25 rem (16.25 pt) line box that gpui-component clips to the field's content box,
/// so the padding must leave at least that much of [`CONTROL_H`]: 24 − 2 × (1 + 2) =
/// 18 pt, with the line centred in it (D26). gpui-component's own medium padding (8 pt)
/// left 8 pt, which cut the text's top and descenders off.
pub const TEXT_FIELD_PAD_Y: Pixels = px(2.);

/// How far a single-line text field's content box (the box its text is clipped to)
/// sits inside its top and bottom edges: the border plus [`TEXT_FIELD_PAD_Y`].
pub fn text_field_inset_y() -> Pixels {
    TEXT_FIELD_BORDER + TEXT_FIELD_PAD_Y
}

/// A single-line text field at the control height with 13 pt text, vertically centred
/// and never clipped.
///
/// The height goes through `Styled::h`: `Input`'s own `h` sets a multi-line input's
/// height only, so on a single-line field it is a no-op and the field stays a 26 pt
/// medium input. The style refinement is applied after the input's size defaults, so
/// these override them.
pub fn text_field(state: &Entity<InputState>) -> Input {
    Styled::h(Input::new(state), CONTROL_H)
        .py(TEXT_FIELD_PAD_Y)
        .text_size(font_size::ROW)
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
    /// The capsule primary button ([`primary_capsule_button`]): the fill beside its
    /// label, full height, starting past the round end (half the height), so no
    /// corner pixel is in it. Reported as the view's `primary` sample.
    PrimaryCapsule,
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
        // vertically; horizontally the region starts past the round end.
        Sample::PrimaryCapsule => Region {
            x: x + h / 2.0 + 1.0,
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
            Sample::PrimaryCapsule => &mut s.primary,
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
    use super::{over, region_for, Sample, CONTROL_H, SAMPLE_MIN};
    use crate::theme::accent::{derive, Rgb, Source};
    use crate::theme::{dark_theme_config, light_theme_config};
    use gpui_kit::component::ThemeMode;
    use gpui_kit::{point, px, size, Bounds, Hsla, Rgba};

    /// A single-line field's text lays out in gpui-component's 1.25 rem line box (its
    /// `Input` `LINE_HEIGHT`), and the rem is the theme's base font size (body, 13 pt).
    /// That box must fit the content box the field clips its text to, or the text's
    /// top and descenders are cut off (rc.4).
    #[test]
    fn a_text_fields_line_fits_inside_its_clip_box() {
        let blue = Source::bare(Rgb::parse("#007aff").unwrap());
        let rem = light_theme_config(&derive(&blue, ThemeMode::Light))
            .font_size
            .expect("the theme sets the base size");
        assert_eq!(
            dark_theme_config(&derive(&blue, ThemeMode::Dark)).font_size,
            Some(rem)
        );
        let line_h = 1.25 * rem;
        let content_h = f32::from(CONTROL_H) - 2. * f32::from(super::text_field_inset_y());
        assert!(
            line_h <= content_h,
            "line {line_h} pt > content box {content_h} pt"
        );
        // The macOS regular control range, at the 13 pt body size.
        assert!((22.0..=28.0).contains(&f32::from(CONTROL_H)));
        assert_eq!(super::font_size::ROW, px(rem));
    }

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
        // A capsule: the region starts past the 12 pt round end and ends before the
        // label (CAPSULE_PAD_X in).
        let capsule = region_for(
            Sample::PrimaryCapsule,
            at(500., 10., 120., f32::from(CONTROL_H)),
        );
        assert!(capsule.w >= SAMPLE_MIN && capsule.h >= SAMPLE_MIN);
        assert!(capsule.x >= 500. + f32::from(CONTROL_H) / 2.);
        assert!(
            capsule.x + capsule.w <= 500. + f32::from(super::CAPSULE_PAD_X),
            "clear of the label: {capsule:?}"
        );
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
