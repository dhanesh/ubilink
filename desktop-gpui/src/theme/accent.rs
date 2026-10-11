//! The system accent colour (D26): where it comes from and the roles derived from it.
//!
//! macOS lets the user pick an accent in System Settings > Appearance; AppKit reports
//! it as `NSColor.controlAccentColor` (the default, and "Multicolor", is blue
//! `#007aff`) and derives `selectedContentBackgroundColor` and
//! `keyboardFocusIndicatorColor` from it. Ubilink follows it live:
//!
//! - [`Source`] is what was read: the accent and, where AppKit gives them, the live
//!   selection and focus colours for each appearance. Release builds read only the
//!   system ([`read_system`]); automation builds take an `accent` fixture first
//!   ([`current`]), so probes stay deterministic on any host.
//! - [`derive`] is pure: it turns a [`Source`] and a mode into the [`Roles`] the theme
//!   uses. It enforces WCAG AA (4.5:1 for button text, 3:1 for a switch track or focus
//!   ring on the card) by the smallest change: darken the fill a little for white text,
//!   or, for a light accent such as orange or yellow, keep the fill and use black text,
//!   as macOS does.
//! - [`observe`] registers for AppKit's colour-change notifications and posts
//!   [`crate::shell::ShellEvent::AccentChanged`]; the shell re-themes on the main
//!   thread through [`crate::theme::refresh_accent`], the same path an automation
//!   `load_fixture {kind: "accent"}` takes.

use gpui_kit::component::ThemeMode;

/// An opaque sRGB colour, 0..=255 per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const WHITE: Rgb = Rgb(255, 255, 255);
    pub const BLACK: Rgb = Rgb(0, 0, 0);

    /// `#rrggbb` (an optional leading `#`), as the fixtures and tokens write it.
    pub fn parse(hex: &str) -> Option<Rgb> {
        let h = hex.trim().trim_start_matches('#');
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgb(ch(0)?, ch(2)?, ch(4)?))
    }

    /// `#rrggbb`. Read by automation's `state` (`appearance.accent`) and the tests.
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    fn luminance(self) -> f64 {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }

    /// `self` at `alpha` composited over the opaque `under`.
    pub fn over(self, alpha: f64, under: Rgb) -> Rgb {
        let mix =
            |t: u8, b: u8| (f64::from(t) * alpha + f64::from(b) * (1.0 - alpha)).round() as u8;
        Rgb(
            mix(self.0, under.0),
            mix(self.1, under.1),
            mix(self.2, under.2),
        )
    }

    /// HSL lightness moved by `delta` (-1..1), hue and saturation kept.
    fn shift_lightness(self, delta: f64) -> Rgb {
        let (h, s, l) = to_hsl(self);
        from_hsl(h, s, (l + delta).clamp(0.0, 1.0))
    }
}

/// WCAG 2 contrast ratio of two opaque colours.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (a.luminance(), b.luminance());
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

fn to_hsl(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (
        f64::from(c.0) / 255.0,
        f64::from(c.1) / 255.0,
        f64::from(c.2) / 255.0,
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |t: f64| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return Rgb(v, v, v);
    }
    Rgb(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// Step `c`'s lightness by 0.001 in `direction` (-1 darker, +1 lighter) until it has
/// `need`:1 against `against`, going at most `limit`. `None` if it never gets there.
fn adjust_until(c: Rgb, against: Rgb, need: f64, direction: f64, limit: f64) -> Option<Rgb> {
    let steps = (limit * 1000.0).round() as u32;
    (0..=steps)
        .map(|i| c.shift_lightness(direction * f64::from(i) / 1000.0))
        .find(|x| contrast(*x, against) >= need)
}

/// What was read: the accent, plus AppKit's own selection and focus colours for each
/// appearance where it gives them (`None` when the source is a bare accent, such as a
/// fixture; [`derive`] then computes them from the accent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    pub accent: Rgb,
    /// `selectedContentBackgroundColor`, light then dark.
    pub selection: [Option<Rgb>; 2],
    /// `keyboardFocusIndicatorColor` (colour, alpha), light then dark.
    pub focus: [Option<(Rgb, f64)>; 2],
}

impl Source {
    pub fn bare(accent: Rgb) -> Self {
        Source {
            accent,
            selection: [None; 2],
            focus: [None; 2],
        }
    }
}

/// Everything the theme takes from the accent, for one mode. Opaque except `ring`
/// and `text_selection`, which carry their alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roles {
    /// The accent as given: what `appearance.accent` reports.
    pub accent: Rgb,
    /// The accent as a non-text mark on the card (checked switch, caret): the accent,
    /// darkened (light) or lightened (dark) only if it is under 3:1 on the card.
    pub mark: Rgb,
    /// The primary button's fill, its text, and its hover and pressed fills.
    pub button: Rgb,
    pub button_text: Rgb,
    pub button_hover: Rgb,
    pub button_active: Rgb,
    /// The focus ring and its alpha (at least 3:1 on the card once composited).
    pub ring: (Rgb, f64),
    /// A selected list row's fill and its text (AA 4.5:1).
    pub selection: Rgb,
    pub selection_text: Rgb,
    /// Selected text inside a text field: the accent at this alpha.
    pub text_selection: (Rgb, f64),
}

/// The opaque card every view paints (`color.windowBackground`), per mode.
pub fn card(mode: ThemeMode) -> Rgb {
    if mode.is_dark() {
        Rgb(0x1e, 0x1e, 0x1e)
    } else {
        Rgb::WHITE
    }
}

/// How far the fill may be darkened for white text before the rule switches to black
/// text on the accent itself (HSL lightness): enough for blue (0.012) and graphite, not
/// enough to turn orange into brown.
const DARKEN_LIMIT: f64 = 0.12;
const AA_TEXT: f64 = 4.5;
const AA_UI: f64 = 3.0;

/// Text for `fill`: white if it reaches 4.5:1, else black if that does, else white.
fn text_on(fill: Rgb) -> Rgb {
    if contrast(Rgb::WHITE, fill) >= AA_TEXT || contrast(Rgb::BLACK, fill) < AA_TEXT {
        Rgb::WHITE
    } else {
        Rgb::BLACK
    }
}

/// The pure derivation (see the module doc).
pub fn derive(source: &Source, mode: ThemeMode) -> Roles {
    let dark = mode.is_dark();
    let ix = usize::from(dark);
    let card = card(mode);
    let accent = source.accent;
    // Toward more contrast with the card: darker on white, lighter on #1e1e1e.
    let away = if dark { 1.0 } else { -1.0 };

    let mark = if contrast(accent, card) >= AA_UI {
        accent
    } else {
        adjust_until(accent, card, AA_UI, away, 1.0).unwrap_or(accent)
    };

    // Primary button: white text if it passes; else darken a little for white text;
    // else (a light accent) black text on the accent; else darken until white passes.
    let (button, button_text) = if contrast(Rgb::WHITE, accent) >= AA_TEXT {
        (accent, Rgb::WHITE)
    } else if let Some(d) = adjust_until(accent, Rgb::WHITE, AA_TEXT, -1.0, DARKEN_LIMIT) {
        (d, Rgb::WHITE)
    } else if contrast(Rgb::BLACK, accent) >= AA_TEXT {
        (accent, Rgb::BLACK)
    } else {
        let d = adjust_until(accent, Rgb::WHITE, AA_TEXT, -1.0, 1.0).unwrap_or(Rgb::BLACK);
        (d, Rgb::WHITE)
    };
    // Hover and pressed shades step away from the text colour, so the label keeps
    // (at least) its contrast.
    let toward_dark = if button_text == Rgb::WHITE { -1.0 } else { 1.0 };
    let button_hover = button.shift_lightness(toward_dark * 0.04);
    let button_active = button.shift_lightness(toward_dark * 0.08);

    // Focus ring: AppKit's own colour and alpha where given, raised until it reaches
    // 3:1 on the card; else the mark colour, opaque.
    let ring = match source.focus[ix] {
        Some((c, a)) => {
            let alpha = (0..=100)
                .map(|i| (a + f64::from(i) / 100.0).min(1.0))
                .find(|al| contrast(c.over(*al, card), card) >= AA_UI)
                .unwrap_or(1.0);
            if contrast(c.over(alpha, card), card) >= AA_UI {
                (c, alpha)
            } else {
                (mark, 1.0)
            }
        }
        None => (mark, 1.0),
    };

    // Selected row: AppKit's selectedContentBackgroundColor where given, else the
    // button fill; text as on a button.
    let selection = source.selection[ix].unwrap_or(button);
    let selection_text = text_on(selection);

    Roles {
        accent,
        mark,
        button,
        button_text,
        button_hover,
        button_active,
        ring,
        selection,
        selection_text,
        text_selection: (accent, 0.3),
    }
}

/// The macOS default accent (blue, also what "Multicolor" reports): the fallback when
/// nothing can be read. `design/ubilink-tokens.json`'s `accent.default`.
pub fn default_source() -> Source {
    Source::bare(
        Rgb::parse(&super::convert::design_tokens().accent.default)
            .expect("accent.default in design/ubilink-tokens.json is #rrggbb"),
    )
}

/// The accent to theme with now: an automation `accent` fixture if one is loaded,
/// else the system's.
pub fn current() -> Source {
    #[cfg(feature = "automation")]
    if let Some(source) = fixture() {
        return source;
    }
    read_system().unwrap_or_else(default_source)
}

/// The `accent` fixture (`{"accent": "#rrggbb"}`), if one is loaded and readable.
#[cfg(feature = "automation")]
fn fixture() -> Option<Source> {
    let path = crate::fixtures::path(crate::fixtures::Fixture::Accent)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| eprintln!("[ubilink] accent fixture {}: {e}", path.display()))
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| eprintln!("[ubilink] accent fixture {}: {e}", path.display()))
        .ok()?;
    let accent = v
        .get("accent")
        .and_then(|a| a.as_str())
        .and_then(Rgb::parse);
    if accent.is_none() {
        eprintln!(
            "[ubilink] accent fixture {}: no #rrggbb accent",
            path.display()
        );
    }
    accent.map(Source::bare)
}

/// Read `controlAccentColor`, and `selectedContentBackgroundColor` and
/// `keyboardFocusIndicatorColor` under the light (Aqua) and dark (DarkAqua)
/// appearances, in sRGB. `None` off the main thread or if AppKit gives no accent.
pub fn read_system() -> Option<Source> {
    let mtm = objc2::MainThreadMarker::new()?;
    crate::shell::native::read_accent(mtm)
}

/// Register for AppKit's accent / system-colour change notifications; each posts
/// [`crate::shell::ShellEvent::AccentChanged`] to `inbox`. Call once, on the main
/// thread.
pub fn observe(inbox: crate::shell::Inbox) {
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        crate::shell::native::observe_accent(mtm, inbox);
    }
}

#[cfg(test)]
mod tests {
    use super::{contrast, derive, Rgb, Source};
    use gpui_kit::component::ThemeMode;

    fn rgb(h: &str) -> Rgb {
        Rgb::parse(h).unwrap()
    }

    /// The probe literals (theming.json, accent.json) are this function's output.
    #[test]
    fn derivation_pins_the_probe_literals() {
        let blue = Source::bare(rgb("#007aff"));
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let r = derive(&blue, mode);
            assert_eq!(
                r.mark.hex(),
                "#007aff",
                "{mode:?}: the switch is the accent"
            );
            assert_eq!(
                r.button.hex(),
                "#0072ef",
                "{mode:?}: darkened for white text"
            );
            assert_eq!(r.button_text, Rgb::WHITE);
            assert_eq!(r.ring, (rgb("#007aff"), 1.0));
        }
        let orange = Source::bare(rgb("#f7821b"));
        let light = derive(&orange, ThemeMode::Light);
        assert_eq!(light.button.hex(), "#f7821b", "orange keeps its fill");
        assert_eq!(light.button_text, Rgb::BLACK, "with black text, as macOS");
        assert_eq!(light.mark.hex(), "#eb7208", "darkened to 3:1 on white");
        let dark = derive(&orange, ThemeMode::Dark);
        assert_eq!(dark.mark.hex(), "#f7821b");
        assert_eq!(
            (dark.button.hex(), dark.button_text),
            ("#f7821b".into(), Rgb::BLACK)
        );
    }

    /// Every derived pair meets AA in both modes, for eight representative accents.
    #[test]
    fn every_accent_meets_aa() {
        // Blue, purple, pink, red, orange, yellow, green, graphite: representative
        // values (this host confirms only blue).
        for accent in [
            "#007aff", "#953d96", "#f74f9e", "#e0383e", "#f7821b", "#ffc600", "#62ba46", "#8c8c8c",
        ] {
            for mode in [ThemeMode::Light, ThemeMode::Dark] {
                let r = derive(&Source::bare(rgb(accent)), mode);
                let card = super::card(mode);
                assert!(
                    contrast(r.button_text, r.button) >= 4.5,
                    "{accent} {mode:?} button"
                );
                assert!(
                    contrast(r.button_text, r.button_hover) >= 4.5,
                    "{accent} hover"
                );
                assert!(
                    contrast(r.button_text, r.button_active) >= 4.5,
                    "{accent} active"
                );
                assert!(contrast(r.mark, card) >= 3.0, "{accent} {mode:?} switch");
                assert!(
                    contrast(r.ring.0.over(r.ring.1, card), card) >= 3.0,
                    "{accent} {mode:?} ring"
                );
                assert!(
                    contrast(r.selection_text, r.selection) >= 4.5,
                    "{accent} selection"
                );
            }
        }
    }

    /// Yellow: white text is hopeless, black passes; the switch darkens on white.
    #[test]
    fn a_light_accent_gets_black_text() {
        let r = derive(&Source::bare(rgb("#ffc600")), ThemeMode::Light);
        assert_eq!((r.button, r.button_text), (rgb("#ffc600"), Rgb::BLACK));
        assert_ne!(r.mark, rgb("#ffc600"));
    }

    /// AppKit's own colours are used where they pass; a 50% focus ring is raised.
    #[test]
    fn live_selection_and_focus_are_used_and_raised() {
        let source = Source {
            accent: rgb("#007aff"),
            selection: [Some(rgb("#0064e1")), Some(rgb("#0059d1"))],
            focus: [Some((rgb("#0067f4"), 0.5)), Some((rgb("#1aa9ff"), 0.5))],
        };
        let light = derive(&source, ThemeMode::Light);
        assert_eq!(light.selection, rgb("#0064e1"));
        assert_eq!(light.selection_text, Rgb::WHITE);
        assert_eq!(light.ring.0, rgb("#0067f4"));
        assert!(
            light.ring.1 > 0.5,
            "0.5 is 2.16:1 on white: {}",
            light.ring.1
        );
        let dark = derive(&source, ThemeMode::Dark);
        assert_eq!(dark.selection, rgb("#0059d1"));
        assert!(dark.ring.1 > 0.5 && dark.ring.1 <= 1.0);
    }

    #[test]
    fn parse_and_hex_round_trip() {
        assert_eq!(rgb("#F7821B").hex(), "#f7821b");
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse("orange"), None);
    }
}
