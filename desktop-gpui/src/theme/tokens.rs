// Satisfies: R12, D26. The token values `ThemeConfig` has no slot for, as constants the
// views use directly: macOS text-style sizes and weights, the 4 pt spacing grid, the
// concentric radii, the glass frame and the native control and row heights. The
// embedded `design/ubilink-tokens.json` is parsed in one place
// (`super::convert::design_tokens`); a unit test pins every constant here to it.
//
// `desktop-gpui` is a `[[bin]]`-only crate, so a `pub` constant no view reads yet is
// "dead" to rustc; `#[allow(dead_code)]` marks that, not unused leftovers.
#![allow(dead_code)]

use gpui_kit::px;
use gpui_kit::{FontWeight, Pixels};

/// The 4 pt spacing grid (`spacing.1`..`spacing.8`), macOS's 4/8 rhythm.
pub mod spacing {
    use super::*;

    pub const S1: Pixels = px(4.);
    pub const S2: Pixels = px(8.);
    pub const S3: Pixels = px(12.);
    pub const S4: Pixels = px(16.);
    pub const S5: Pixels = px(20.);
    pub const S6: Pixels = px(24.);
    pub const S7: Pixels = px(32.);
    pub const S8: Pixels = px(40.);
}

/// Concentric corners (macOS 27): the glass panel, the card inset in it by
/// [`size::FRAME`] (panel − frame), and controls and rows inside the card.
pub mod radius {
    use super::*;

    pub const PANEL: Pixels = px(18.);
    pub const CARD: Pixels = px(14.);
    pub const CONTROL: Pixels = px(6.);
    pub const ROW: Pixels = px(6.);
    /// "Round no matter the size", as `Theme::radius_full()`.
    pub const PILL: Pixels = px(999.);
}

/// macOS text styles by role: DISPLAY = title2, TITLE = headline (with
/// [`font_weight::HEADLINE`]), ROW = body, META = callout. The base size (body, 13)
/// is `ThemeConfig::font_size`, the gpui-component rem.
pub mod font_size {
    use super::*;

    pub const DISPLAY: Pixels = px(17.);
    pub const TITLE: Pixels = px(13.);
    pub const ROW: Pixels = px(13.);
    pub const META: Pixels = px(12.);
}

/// macOS text-style weights: body text regular, headline bold.
pub mod font_weight {
    use super::*;

    pub const REGULAR: FontWeight = FontWeight(400.);
    pub const HEADLINE: FontWeight = FontWeight(700.);
}

/// Native sizes: the glass frame around the card, push buttons and text fields (24
/// pt, macOS's regular control size), and list rows.
pub mod size {
    use super::*;

    pub const FRAME: Pixels = px(4.);
    pub const CONTROL: Pixels = px(24.);
    pub const ROW: Pixels = px(24.);
    /// The link browser's two-line row: the title over the URL (Things-style).
    pub const ROW_TWO_LINE: Pixels = px(40.);
    /// A Notes-style form's label column (Settings): labels end on its right edge.
    pub const FORM_LABEL: Pixels = px(112.);
}

/// The first real family in `font.mono`, for URIs (identifiers). The theme's
/// `mono_font_family` stays gpui-component's default.
pub const MONO_FONT_FAMILY: &str = "SF Mono";

#[cfg(test)]
mod tests {
    use crate::theme::convert::design_tokens;
    use gpui_kit::px;

    /// Pins every constant above to the JSON it was copied from.
    #[test]
    fn constants_match_the_embedded_json() {
        let t = design_tokens();

        assert_eq!(super::spacing::S1, px(t.spacing.s1));
        assert_eq!(super::spacing::S2, px(t.spacing.s2));
        assert_eq!(super::spacing::S3, px(t.spacing.s3));
        assert_eq!(super::spacing::S4, px(t.spacing.s4));
        assert_eq!(super::spacing::S5, px(t.spacing.s5));
        assert_eq!(super::spacing::S6, px(t.spacing.s6));
        assert_eq!(super::spacing::S7, px(t.spacing.s7));
        assert_eq!(super::spacing::S8, px(t.spacing.s8));

        assert_eq!(super::radius::PANEL, px(t.radius.panel));
        assert_eq!(super::radius::CARD, px(t.radius.card));
        assert_eq!(super::radius::CONTROL, px(t.radius.control));
        assert_eq!(super::radius::ROW, px(t.radius.row));
        assert_eq!(super::radius::PILL, px(t.radius.pill));
        // Concentric: the card's corner is the panel's minus the frame.
        assert_eq!(
            super::radius::CARD,
            super::radius::PANEL - super::size::FRAME
        );

        assert_eq!(super::font_size::DISPLAY, px(t.font_size.display));
        assert_eq!(super::font_size::TITLE, px(t.font_size.title));
        assert_eq!(super::font_size::ROW, px(t.font_size.row));
        assert_eq!(super::font_size::META, px(t.font_size.meta));
        assert_eq!(super::font_weight::REGULAR.0, t.font_weight.regular);
        assert_eq!(super::font_weight::HEADLINE.0, t.font_weight.headline);

        assert_eq!(super::size::FRAME, px(t.size.frame));
        assert_eq!(super::size::CONTROL, px(t.size.control));
        assert_eq!(super::size::ROW, px(t.size.row));
        assert_eq!(super::size::ROW_TWO_LINE, px(t.size.row_two_line));
        assert_eq!(super::size::FORM_LABEL, px(t.size.form_label));

        assert_eq!(
            super::MONO_FONT_FAMILY,
            crate::theme::convert::first_real_font_family(&t.font.mono)
        );
    }
}
