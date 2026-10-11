//! Ubilink's theme: native macOS 27 (D26). `design/ubilink-tokens.json` (AppKit's
//! system colours, macOS text-style sizes, radii) and the live system accent
//! ([`accent`]) become gpui-component `ThemeConfig`s for light and dark mode. What
//! `ThemeConfig` has no slot for (text-style sizes and weights, spacing, the glass
//! frame, control and row heights, the selected-row fill) is exposed through
//! [`tokens`] and [`roles`] for the views.
//!
//! # Mapping table
//!
//! | Token / role | gpui-component field | Note |
//! |---|---|---|
//! | `color.windowBackground` | `background` (alpha 0), `popover` | the card's RGB; views paint it opaque (`theming::surface`), the window root stays clear so the glass shows |
//! | `color.label` | `foreground`, `popover_foreground`, `secondary_foreground`, `accent_foreground` | |
//! | `color.secondaryLabel` | `muted_foreground` | light raised to 0.55 for AA (on the card and on a hovered row) |
//! | `color.underPageBackground` | `muted` | |
//! | `color.separator` | `border`, `table_row_border` | hairlines (decorative) |
//! | `color.tertiaryLabel` | `scrollbar_thumb` | |
//! | `color.controlBorder` | `input` | a text field's edge, raised to 3:1 |
//! | `color.control` / `controlHover` / `controlActive` | `secondary` / `secondary_hover` / `secondary_active` | push buttons (`Button::secondary`) |
//! | `color.rowHover` | `accent`, `list_hover` | gpui-component's `accent` is a hover surface, not the system accent |
//! | — | `list_active`, `list_active_border` | transparent: views draw the selection ([`selection`]) |
//! | `color.switchOff` / `switchThumb` | `switch` / `switch_thumb` | off track raised to 3:1 |
//! | accent `mark` | `primary`, `caret`, `link` | a checked switch takes `primary` |
//! | accent `button*` | `button_primary*`, `primary_foreground`, `primary_hover`, `primary_active` | the default (primary) button |
//! | accent `ring` | `ring` | focus ring, alpha raised to 3:1 |
//! | accent `text_selection` | `selection` | selected text in a field |
//! | `color.danger` / `dangerFill` | `danger` / `button_danger` (+ white foregrounds) | |
//! | `color.success`, `color.warning` | `success`, `warning` | AA-darkened in light mode |
//! | `fontSize.body` | `font_size` | 13, macOS body and the rem |
//! | `font.ui` | `font_family` | `.SystemUIFont` (SF) |
//! | `radius.control` / `radius.card` | `radius` / `radius_lg` | 6 / 14 |

pub mod accent;
mod convert;
pub mod tokens;

pub use convert::{dark_theme_config, light_theme_config};

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Rgba};

/// The accent roles the theme was last built from, both modes: views read the
/// selected-row fill from here ([`selection`]); automation reports the accent.
pub struct AccentRoles {
    pub light: accent::Roles,
    pub dark: accent::Roles,
}

impl Global for AccentRoles {}

/// Installs ubilink's light and dark themes for the current system accent and applies
/// whichever matches the current appearance. Called once from `main.rs` after
/// `gpui_kit::init(cx)` (which installs gpui-component's defaults first).
pub fn install(cx: &mut App) {
    apply(accent::current(), cx);
    sync(cx);
}

/// Rebuild both themes from `source` and keep the current mode.
fn apply(source: accent::Source, cx: &mut App) {
    let light = accent::derive(&source, ThemeMode::Light);
    let dark = accent::derive(&source, ThemeMode::Dark);
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light_theme_config(&light));
        theme.dark_theme = Rc::new(dark_theme_config(&dark));
    });
    cx.set_global(AccentRoles { light, dark });
}

/// How many accent re-themes have run (`refresh_accent`), for automation's `state`.
static ACCENT_REFRESHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg_attr(not(feature = "automation"), allow(dead_code))]
pub fn accent_refreshes() -> u64 {
    ACCENT_REFRESHES.load(std::sync::atomic::Ordering::Relaxed)
}

/// An opaque theme colour as `#rrggbb` (automation's `state`).
#[cfg_attr(not(feature = "automation"), allow(dead_code))]
pub fn hex_of(c: Hsla) -> String {
    let c = Rgba::from(c);
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    accent::Rgb(ch(c.r), ch(c.g), ch(c.b)).hex()
}

/// The system accent (or, in automation builds, the `accent` fixture) changed: rebuild
/// the themes and repaint open windows in place, in the mode they are in.
pub fn refresh_accent(cx: &mut App) {
    ACCENT_REFRESHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    apply(accent::current(), cx);
    let mode = cx.theme().mode;
    Theme::change(mode, None, cx);
    cx.refresh_windows();
}

/// Apply the light or dark theme matching the app's current appearance. Split out of
/// `install` so the automation `set_appearance system` verb re-syncs through it.
pub fn sync(cx: &mut App) {
    let mode = ThemeMode::from(cx.window_appearance());
    Theme::change(mode, None, cx);
}

/// The accent roles for the theme's current mode.
pub fn roles(cx: &App) -> accent::Roles {
    let r = cx.global::<AccentRoles>();
    if cx.theme().mode.is_dark() {
        r.dark
    } else {
        r.light
    }
}

fn hsla(c: accent::Rgb) -> Hsla {
    Hsla::from(Rgba {
        r: f32::from(c.0) / 255.0,
        g: f32::from(c.1) / 255.0,
        b: f32::from(c.2) / 255.0,
        a: 1.0,
    })
}

/// A selected list row's fill and text (macOS `selectedContentBackgroundColor` with
/// white text, or what the accent derivation gives for a light accent).
pub fn selection(cx: &App) -> (Hsla, Hsla) {
    let r = roles(cx);
    (hsla(r.selection), hsla(r.selection_text))
}

#[cfg(test)]
mod tests {
    use super::accent::{derive, Rgb, Source};
    use super::convert::to_hex_color;
    use super::*;

    /// The live theme colours automation reports survive the hex -> Hsla -> hex trip
    /// exactly, so the accent probe can compare them as strings.
    #[test]
    fn hex_of_round_trips_the_probe_colours() {
        for h in ["#007aff", "#0072ef", "#f7821b", "#eb7208"] {
            let c = Hsla::from(Rgba::try_from(h).unwrap());
            assert_eq!(hex_of(c), h);
        }
    }

    /// A token colour (`rgba(..)` / `#rrggbb`) as (RGB, alpha).
    fn rgba(value: &str) -> (Rgb, f64) {
        let hex = to_hex_color(value);
        let rgb = Rgb::parse(&hex[..7]).unwrap();
        let a = if hex.len() == 9 {
            f64::from(u8::from_str_radix(&hex[7..9], 16).unwrap()) / 255.0
        } else {
            1.0
        };
        (rgb, a)
    }

    /// D26 AA: text roles on the card, on the under-page surface and on a hovered row
    /// (the browser's URI is muted text on a hovered ListItem), in both modes.
    #[test]
    fn text_on_every_surface_meets_aa() {
        use super::accent::{card, contrast};
        let t = super::convert::design_tokens();
        let c = &t.color;
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let pick = |d: &super::convert::DualValue| {
                rgba(if mode.is_dark() { &d.dark } else { &d.light })
            };
            let card = card(mode);
            let under = pick(&c.under_page_background).0;
            let (hover, ha) = pick(&c.row_hover);
            let hovered = hover.over(ha, card);
            let surfaces = [("card", card), ("underPage", under), ("rowHover", hovered)];
            // Label roles appear on every surface; status lines only on the card
            // (no view puts one in a row or on the under-page surface).
            for (name, (fg, fa), on) in [
                ("label", pick(&c.label), &surfaces[..]),
                ("secondaryLabel", pick(&c.secondary_label), &surfaces[..]),
                ("danger", pick(&c.danger), &surfaces[..1]),
                ("success", pick(&c.success), &surfaces[..1]),
                ("warning", pick(&c.warning), &surfaces[..1]),
            ] {
                for (surface, bg) in on {
                    let r = contrast(fg.over(fa, *bg), *bg);
                    assert!(r >= 4.5, "{mode:?} {name} on {surface}: {r:.2}");
                }
            }
            for fill in [&c.danger_fill, &c.danger_fill_hover, &c.danger_fill_active] {
                let r = contrast(Rgb::WHITE, pick(fill).0);
                assert!(r >= 4.5, "{mode:?} white on a destructive fill: {r:.2}");
            }
        }
    }

    /// RED/GREEN anchor (R12, D26): concrete values from `design/ubilink-tokens.json`
    /// and the accent derivation land in the converted `ThemeConfig`s.
    #[test]
    fn theme_tokens_convert() {
        let blue = Source::bare(Rgb::parse("#007aff").unwrap());
        let light = light_theme_config(&derive(&blue, ThemeMode::Light));
        let dark = dark_theme_config(&derive(&blue, ThemeMode::Dark));

        // windowBackgroundColor at alpha 0: the window root lets the glass show and
        // each view paints the card opaque from the same RGB.
        assert_eq!(light.colors.background.as_deref(), Some("#FFFFFF00"));
        assert_eq!(dark.colors.background.as_deref(), Some("#1E1E1E00"));
        // labelColor, and secondaryLabelColor raised for AA in light mode.
        assert_eq!(
            light.colors.foreground.as_deref(),
            Some(to_hex_color("rgba(0,0,0,0.85)").as_str())
        );
        assert_eq!(light.colors.muted_foreground.as_deref(), Some("#0000008C"));
        assert_eq!(dark.colors.muted_foreground.as_deref(), Some("#FFFFFF8C"));

        for c in [&light, &dark] {
            assert_eq!(c.font_family.as_deref(), Some(".SystemUIFont"));
            assert_eq!(c.font_size, Some(13.0));
            assert_eq!((c.radius, c.radius_lg), (Some(6), Some(14)));
            // The accent: a checked switch takes `primary`; the default button is
            // darkened for white text; the ring is the accent, opaque.
            assert_eq!(c.colors.primary.as_deref(), Some("#007AFF"));
            assert_eq!(c.colors.button_primary.as_deref(), Some("#0072EF"));
            assert_eq!(
                c.colors.button_primary_foreground.as_deref(),
                Some("#FFFFFF")
            );
            assert_eq!(c.colors.ring.as_deref(), Some("#007AFFFF"));
            assert_eq!(c.colors.list_active.as_deref(), Some("#00000000"));
            assert_eq!(
                c.colors.button_danger_foreground.as_deref(),
                Some("#FFFFFF")
            );
        }
        // Status colours, AA-darkened in light mode.
        assert_eq!(light.colors.danger.as_deref(), Some("#EE0005"));
        assert_eq!(dark.colors.danger.as_deref(), Some("#FF4245"));
        assert_eq!(light.colors.success.as_deref(), Some("#24883D"));
        assert_eq!(light.colors.button_danger.as_deref(), Some("#EE0005"));
    }
}
