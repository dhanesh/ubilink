// Satisfies: R12 (this module), consumed by Phase 1 R14 views.
//
// gpui-component's `ThemeConfig` (see `super::convert`) has a slot for one
// radius pair, one font size and a fixed color palette — ubilink's tokens
// carry more than that (a 4pt spacing scale, five radii, five font sizes, and
// five `color.*` entries with no matching `ThemeConfigColors` field). Phase 1
// views read those directly from here instead of recomputing them from JSON,
// so the embedded `design/ubilink-tokens.json` is parsed in exactly one
// place (`super::convert::design_tokens`).
//
// These are `const`, not values pulled from `design_tokens()` at run time: the
// source JSON is static (embedded at compile time, never read from disk), so
// there is nothing to resolve twice. A unit test below pins each constant to
// its JSON source so the two cannot drift silently.
//
// `desktop-gpui` is a `[[bin]]`-only crate (no lib target), so rustc's
// dead-code analysis treats every `pub` item here as unreachable until a
// Phase 1 view (R14) actually calls it — there is no external crate for
// which "pub" could mean "used". `#[allow(dead_code)]` below is that:
// deliberately public ahead of its first caller, not unused leftovers.
#![allow(dead_code)]

use gpui_kit::px;
use gpui_kit::Pixels;

/// The 4pt spacing scale (`spacing.1`..`spacing.8`).
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

/// Radii Task 12's converter doesn't spend on `ThemeConfig::radius`/`radius_lg`
/// (those two take `radius.control` and `radius.panel`; the rest have no slot).
pub mod radius {
    use super::*;

    pub const CARD: Pixels = px(12.);
    pub const ROW: Pixels = px(10.);
    pub const TILE: Pixels = px(7.);
    pub const KBD: Pixels = px(6.);
    /// `radius.pill` (999) is a "round no matter the size" sentinel, the same
    /// role `Theme::radius_full()` plays for gpui-component's own radius.
    pub const PILL: Pixels = px(999.);
}

/// Font sizes Task 12's converter doesn't spend on `ThemeConfig::font_size`
/// (that one takes `fontSize.body`; the rest have no slot).
pub mod font_size {
    use super::*;

    pub const DISPLAY: Pixels = px(18.);
    pub const TITLE: Pixels = px(16.);
    pub const ROW: Pixels = px(14.);
    pub const META: Pixels = px(12.);
}

/// The first real family in `font.mono` (the same "skip the leading CSS
/// alias" rule `convert::first_real_font_family` applies to `font.ui`).
/// `ThemeConfig::mono_font_family` is left at gpui-component's own default
/// (see Task 12's report) rather than set from this constant: ubilink's mono
/// stack exists for the web shell's `<code>`/URL rendering, and no Phase 0/1
/// GPUI view renders monospace text yet.
pub const MONO_FONT_FAMILY: &str = "SF Mono";

/// `color.*` tokens with no `ThemeConfigColors` field, by theme mode. Each is
/// the exact CSS value from `design/ubilink-tokens.json` (not hex-converted:
/// `uiShadow` isn't a color at all, and the rest have no gpui-component
/// consumer to convert for yet).
pub mod unmapped_colors {
    /// `color.uiShadow` — a CSS `box-shadow` string. `ThemeConfig` only has a
    /// `shadow: bool` toggle, no shadow geometry/color slot.
    pub mod ui_shadow {
        pub const DARK: &str = "0 28px 64px -18px rgba(0,0,0,0.62), 0 2px 10px rgba(0,0,0,0.40)";
        pub const LIGHT: &str = "0 28px 64px -18px rgba(0,0,0,0.24), 0 2px 10px rgba(0,0,0,0.10)";
    }

    /// `color.text3` — a third, lowest-contrast text tier. `ThemeConfigColors`
    /// models exactly two (`foreground`, `muted_foreground`).
    pub mod text3 {
        pub const DARK: &str = "rgba(235,235,245,0.34)";
        pub const LIGHT: &str = "rgba(60,60,67,0.40)";
    }

    /// `color.kbdBg` — the keycap chip's background. No `ThemeConfigColors`
    /// field renders the `Kbd` component specifically.
    pub mod kbd_bg {
        pub const DARK: &str = "rgba(255,255,255,0.08)";
        pub const LIGHT: &str = "rgba(0,0,0,0.05)";
    }

    /// `color.kbdBorder` — the keycap chip's border.
    pub mod kbd_border {
        pub const DARK: &str = "rgba(255,255,255,0.12)";
        pub const LIGHT: &str = "rgba(0,0,0,0.12)";
    }

    /// `color.kbdText` — the keycap chip's text.
    pub mod kbd_text {
        pub const DARK: &str = "rgba(235,235,245,0.60)";
        pub const LIGHT: &str = "rgba(60,60,67,0.60)";
    }
}

#[cfg(test)]
mod tests {
    use crate::theme::convert::design_tokens;
    use gpui_kit::px;

    /// Pins every constant above to the JSON it was copied from, so an edit to
    /// `design/ubilink-tokens.json` that doesn't also update this file fails
    /// here instead of shipping a silently stale constant.
    #[test]
    fn constants_match_the_embedded_json() {
        let tokens = design_tokens();

        assert_eq!(super::spacing::S1, px(tokens.spacing.s1));
        assert_eq!(super::spacing::S2, px(tokens.spacing.s2));
        assert_eq!(super::spacing::S3, px(tokens.spacing.s3));
        assert_eq!(super::spacing::S4, px(tokens.spacing.s4));
        assert_eq!(super::spacing::S5, px(tokens.spacing.s5));
        assert_eq!(super::spacing::S6, px(tokens.spacing.s6));
        assert_eq!(super::spacing::S7, px(tokens.spacing.s7));
        assert_eq!(super::spacing::S8, px(tokens.spacing.s8));

        assert_eq!(super::radius::CARD, px(tokens.radius.card));
        assert_eq!(super::radius::ROW, px(tokens.radius.row));
        assert_eq!(super::radius::TILE, px(tokens.radius.tile));
        assert_eq!(super::radius::KBD, px(tokens.radius.kbd));
        assert_eq!(super::radius::PILL, px(tokens.radius.pill));

        assert_eq!(super::font_size::DISPLAY, px(tokens.font_size.display));
        assert_eq!(super::font_size::TITLE, px(tokens.font_size.title));
        assert_eq!(super::font_size::ROW, px(tokens.font_size.row));
        assert_eq!(super::font_size::META, px(tokens.font_size.meta));

        assert_eq!(
            super::MONO_FONT_FAMILY,
            crate::theme::convert::first_real_font_family(&tokens.font.mono)
        );

        assert_eq!(super::unmapped_colors::text3::DARK, tokens.color.text3.dark);
        assert_eq!(
            super::unmapped_colors::text3::LIGHT,
            tokens.color.text3.light
        );
        assert_eq!(
            super::unmapped_colors::ui_shadow::DARK,
            tokens.color.ui_shadow.dark
        );
        assert_eq!(
            super::unmapped_colors::ui_shadow::LIGHT,
            tokens.color.ui_shadow.light
        );
        assert_eq!(
            super::unmapped_colors::kbd_bg::DARK,
            tokens.color.kbd_bg.dark
        );
        assert_eq!(
            super::unmapped_colors::kbd_bg::LIGHT,
            tokens.color.kbd_bg.light
        );
        assert_eq!(
            super::unmapped_colors::kbd_border::DARK,
            tokens.color.kbd_border.dark
        );
        assert_eq!(
            super::unmapped_colors::kbd_border::LIGHT,
            tokens.color.kbd_border.light
        );
        assert_eq!(
            super::unmapped_colors::kbd_text::DARK,
            tokens.color.kbd_text.dark
        );
        assert_eq!(
            super::unmapped_colors::kbd_text::LIGHT,
            tokens.color.kbd_text.light
        );
    }
}
