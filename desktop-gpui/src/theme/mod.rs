//! Converts ubilink's web design tokens (`design/ubilink-tokens.json`, also
//! rendered as `dist/tokens.css` for the Tauri shell) into gpui-component
//! `ThemeConfig`s for light and dark mode (Phase 0 R12). Everything the
//! tokens define but gpui-component's `ThemeConfig`/`ThemeConfigColors` have
//! no slot for — the spacing scale, the radii/font sizes beyond the one pair
//! `ThemeConfig` takes, and five `color.*` entries — is exposed as plain Rust
//! constants in [`tokens`] for Phase 1 (R14) views to use directly.
//!
//! The JSON is embedded with `include_str!` at compile time (see
//! [`convert::design_tokens`]), so the shipped binary has no runtime
//! dependency on `design/`.
//!
//! # Mapping table
//!
//! `color.*` token → `ThemeConfigColors` field (serde key in parens). Every
//! `color.*` entry in `design/ubilink-tokens.json` is listed; "unmapped"
//! means `ThemeConfigColors` has no field for it — those are exposed verbatim
//! (both modes) as constants in [`tokens::unmapped_colors`] instead.
//!
//! | Design token | gpui-component field | Note |
//! |---|---|---|
//! | `color.uiBg` | `background` | rgba → hex |
//! | `color.uiBg2` | `popover` (`popover.background`) | secondary/raised surface |
//! | `color.uiBorder` | `border` | |
//! | `color.uiHairline` | `table_row_border` (`table.row.border`) | subtlest divider gpui-component names |
//! | `color.uiShadow` | *unmapped* | CSS `box-shadow` string, not a color; `ThemeConfig.shadow` is a bool toggle, no geometry/color slot |
//! | `color.text1` | `foreground`, `accent_foreground` (`accent.foreground`) | primary text; accent-background hover states keep normal text color |
//! | `color.text2` | `muted_foreground` (`muted.foreground`) | |
//! | `color.text3` | *unmapped* | `ThemeConfigColors` models exactly two text tiers |
//! | `color.accent` | `accent` (`accent.background`), `primary` (`primary.background`) | one accent (macOS system blue) serves both roles in these tokens |
//! | `color.accentSoft` | `selection` (`selection.background`) | |
//! | `color.rowSelected` | `list_active` (`list.active.background`) | |
//! | `color.rowHover` | `list_hover` (`list.hover.background`) | |
//! | `color.ctrlBg` | `secondary` (`secondary.background`), `muted` (`muted.background`) | control surface doubles as the generic muted surface (Skeleton/Switch) |
//! | `color.ctrlBgHover` | `secondary_hover` (`secondary.hover.background`) | |
//! | `color.ctrlBorder` | `input` (`input.border`) | |
//! | `color.kbdBg` | *unmapped* | no `Kbd`-specific field |
//! | `color.kbdBorder` | *unmapped* | no `Kbd`-specific field |
//! | `color.kbdText` | *unmapped* | no `Kbd`-specific field |
//!
//! Other fields:
//!
//! | Design token | gpui-component field | Note |
//! |---|---|---|
//! | `status.green` | `success` (`success.background`) | theme-independent (one value, not dark/light) |
//! | `status.danger` | `danger` (`danger.background`) | theme-independent |
//! | `radius.control` | `ThemeConfig.radius` | |
//! | `radius.panel` | `ThemeConfig.radius_lg` | |
//! | `fontSize.body` | `ThemeConfig.font_size` | |
//! | `font.ui` (first quoted family) | `ThemeConfig.font_family` | `-apple-system` leads the stack as a CSS alias, not an installed name; `"SF Pro Text"` is the first real one |
//! | — (not a token) | `primary_foreground` (`primary.foreground`) | hardcoded `#FFFFFF`: native macOS pairs a blue primary action with white text; the tokens define no such value |
//!
//! `radius.{card,row,tile,kbd,pill}`, `fontSize.{display,title,row,meta}`,
//! `font.mono` and `spacing.*` have no `ThemeConfig` slot at all; see
//! [`tokens`].

mod convert;
pub mod tokens;

pub use convert::{dark_theme_config, light_theme_config};

use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::App;

/// Installs ubilink's light and dark themes as the app's `Theme::light_theme`/
/// `dark_theme` and applies whichever matches the current system appearance.
///
/// Called once from `main.rs` after `gpui_kit::init(cx)` (which installs
/// gpui-component's own default themes first — this call replaces them).
/// There is no window yet in Phase 0, so appearance comes from
/// `cx.window_appearance()` rather than a window's own (the same fallback
/// `Theme::sync_system_appearance` uses when passed `None`); Phase 1's window
/// can resync against its own `Window::appearance()` if it ever needs to
/// react to an appearance change after launch.
pub fn install(cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light_theme_config());
        theme.dark_theme = Rc::new(dark_theme_config());
    });

    let mode = ThemeMode::from(cx.window_appearance());
    Theme::change(mode, None, cx);
}

#[cfg(test)]
mod tests {
    use super::convert::to_hex_color;
    use super::*;

    /// RED/GREEN anchor for Task 12: concrete values from
    /// `design/ubilink-tokens.json` must land in the converted `ThemeConfig`s.
    /// Run with:
    /// `cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation theme_tokens_convert`
    #[test]
    fn theme_tokens_convert() {
        let dark = dark_theme_config();
        let light = light_theme_config();

        // `color.uiBg`: rgba(28,28,30,0.72) / rgba(250,250,252,0.72) → hex with alpha.
        assert_eq!(
            dark.colors.background.as_deref(),
            Some(to_hex_color("rgba(28,28,30,0.72)").as_str())
        );
        assert_eq!(dark.colors.background.as_deref(), Some("#1C1C1EB8"));
        assert_eq!(
            light.colors.background.as_deref(),
            Some(to_hex_color("rgba(250,250,252,0.72)").as_str())
        );
        assert_eq!(light.colors.background.as_deref(), Some("#FAFAFCB8"));

        // radius.control → radius; radius.panel → radius_lg (theme-independent,
        // so both modes must agree).
        assert_eq!(dark.radius, Some(8));
        assert_eq!(light.radius, Some(8));
        assert_eq!(dark.radius_lg, Some(17));
        assert_eq!(light.radius_lg, Some(17));

        // fontSize.body → font_size.
        assert_eq!(dark.font_size, Some(13.0));
        assert_eq!(light.font_size, Some(13.0));

        // font.ui's first quoted (real) family → font_family.
        assert_eq!(dark.font_family.as_deref(), Some("SF Pro Text"));
        assert_eq!(light.font_family.as_deref(), Some("SF Pro Text"));

        // color.accent feeds both accent and primary (one accent, two roles).
        assert_eq!(dark.colors.accent.as_deref(), Some("#0A84FF"));
        assert_eq!(dark.colors.primary.as_deref(), Some("#0A84FF"));
        assert_eq!(light.colors.accent.as_deref(), Some("#007AFF"));
        assert_eq!(light.colors.primary.as_deref(), Some("#007AFF"));

        // status.* → success/danger (theme-independent, same hex either mode).
        assert_eq!(dark.colors.success.as_deref(), Some("#30d158"));
        assert_eq!(light.colors.success.as_deref(), Some("#30d158"));
        assert_eq!(dark.colors.danger.as_deref(), Some("#ff453a"));
        assert_eq!(light.colors.danger.as_deref(), Some("#ff453a"));

        // Every color.* token with dark/light values maps to a
        // ThemeConfigColors field, or is named in `tokens::unmapped_colors`
        // (both checked against the embedded JSON by
        // `tokens::tests::constants_match_the_embedded_json`).
        let mapped = [
            dark.colors.background.is_some(),
            dark.colors.popover.is_some(),
            dark.colors.border.is_some(),
            dark.colors.table_row_border.is_some(),
            dark.colors.foreground.is_some(),
            dark.colors.accent_foreground.is_some(),
            dark.colors.muted_foreground.is_some(),
            dark.colors.accent.is_some(),
            dark.colors.primary.is_some(),
            dark.colors.selection.is_some(),
            dark.colors.list_active.is_some(),
            dark.colors.list_hover.is_some(),
            dark.colors.secondary.is_some(),
            dark.colors.muted.is_some(),
            dark.colors.secondary_hover.is_some(),
            dark.colors.input.is_some(),
        ];
        assert!(
            mapped.iter().all(|present| *present),
            "every color.* token's target ThemeConfigColors field must be set: {mapped:?}"
        );
        // uiShadow, text3, kbdBg, kbdBorder, kbdText: intentionally unmapped
        // (see the module doc table); their raw values live in
        // tokens::unmapped_colors.
        assert_eq!(
            tokens::unmapped_colors::text3::DARK,
            "rgba(235,235,245,0.34)"
        );
        assert_eq!(
            tokens::unmapped_colors::kbd_text::LIGHT,
            "rgba(60,60,67,0.60)"
        );
    }
}
