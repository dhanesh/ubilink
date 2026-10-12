// Satisfies: R12 (design tokens become gpui-component ThemeConfigs), D26 (native macOS
// 27). See desktop-gpui/src/theme/mod.rs for the mapping table this module implements.
//
// `design/ubilink-tokens.json` is embedded at compile time (`include_str!`) so the
// shipped binary has no runtime file dependency on `design/`.

use gpui_kit::component::{ThemeConfig, ThemeConfigColors, ThemeMode};
use gpui_kit::SharedString;
use serde::Deserialize;

use super::accent::{Rgb, Roles};

/// The source of truth, embedded so the GPUI binary ships with no runtime dependency
/// on `design/`.
const RAW_TOKENS: &str = include_str!("../../../design/ubilink-tokens.json");

/// Mirrors `design/ubilink-tokens.json`'s shape. `$meta` and each colour's `aa` note
/// are documentation and are not typed.
#[derive(Debug, Deserialize)]
pub(super) struct DesignTokens {
    pub(super) font: FontTokens,
    #[serde(rename = "fontSize")]
    pub(super) font_size: FontSizeTokens,
    // Read only by `tokens::tests` (pins `tokens::font_weight`).
    #[allow(dead_code)]
    #[serde(rename = "fontWeight")]
    pub(super) font_weight: FontWeightTokens,
    pub(super) radius: RadiusTokens,
    // Read only by `tokens::tests`, which pins `tokens::spacing` against it.
    #[allow(dead_code)]
    pub(super) spacing: SpacingTokens,
    #[allow(dead_code)]
    pub(super) size: SizeTokens,
    pub(super) accent: AccentTokens,
    pub(super) color: ColorTokens,
}

#[derive(Debug, Deserialize)]
pub(super) struct FontTokens {
    /// The UI family GPUI resolves: `.SystemUIFont` is the platform UI font (SF).
    pub(super) ui: String,
    /// Read only by `tokens::tests` (pins `tokens::MONO_FONT_FAMILY`).
    #[allow(dead_code)]
    pub(super) mono: String,
}

/// macOS text styles: display = title2 (17), title = headline (13, bold), row =
/// body (13), body = the base size (13, the gpui-component rem), meta = callout (12).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct FontSizeTokens {
    pub(super) display: f32,
    pub(super) title: f32,
    pub(super) row: f32,
    pub(super) body: f32,
    pub(super) meta: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct FontWeightTokens {
    pub(super) regular: f32,
    pub(super) headline: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct RadiusTokens {
    pub(super) panel: f32,
    pub(super) card: f32,
    pub(super) control: f32,
    pub(super) row: f32,
    pub(super) pill: f32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct SpacingTokens {
    #[serde(rename = "1")]
    pub(super) s1: f32,
    #[serde(rename = "2")]
    pub(super) s2: f32,
    #[serde(rename = "3")]
    pub(super) s3: f32,
    #[serde(rename = "4")]
    pub(super) s4: f32,
    #[serde(rename = "5")]
    pub(super) s5: f32,
    #[serde(rename = "6")]
    pub(super) s6: f32,
    #[serde(rename = "7")]
    pub(super) s7: f32,
    #[serde(rename = "8")]
    pub(super) s8: f32,
}

/// The glass frame around the card, and the control and list-row heights.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct SizeTokens {
    pub(super) frame: f32,
    pub(super) control: f32,
    pub(super) row: f32,
    /// The link browser's two-line row (title over URL).
    #[serde(rename = "rowTwoLine")]
    pub(super) row_two_line: f32,
}

#[derive(Debug, Deserialize)]
pub(super) struct AccentTokens {
    /// The macOS default accent, used when the system's cannot be read.
    pub(super) default: String,
}

/// One `color.*` entry: a value per mode (`rgba(...)` or `#rrggbb`).
#[derive(Debug, Clone, Deserialize)]
pub(super) struct DualValue {
    pub(super) dark: String,
    pub(super) light: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ColorTokens {
    pub(super) window_background: DualValue,
    pub(super) under_page_background: DualValue,
    pub(super) label: DualValue,
    pub(super) secondary_label: DualValue,
    #[allow(dead_code)]
    pub(super) tertiary_label: DualValue,
    pub(super) separator: DualValue,
    pub(super) control_border: DualValue,
    pub(super) control: DualValue,
    pub(super) control_hover: DualValue,
    pub(super) control_active: DualValue,
    pub(super) row_hover: DualValue,
    pub(super) row_selected: DualValue,
    pub(super) switch_off: DualValue,
    pub(super) switch_thumb: DualValue,
    pub(super) danger: DualValue,
    pub(super) danger_fill: DualValue,
    pub(super) danger_fill_hover: DualValue,
    pub(super) danger_fill_active: DualValue,
    pub(super) success: DualValue,
    pub(super) warning: DualValue,
}

/// Parses the embedded tokens. Cheap (a few KB of JSON), so no `OnceLock`: a parse
/// failure panics at the call that is wrong, which `cargo test` covers.
pub(super) fn design_tokens() -> DesignTokens {
    serde_json::from_str(RAW_TOKENS).expect(
        "design/ubilink-tokens.json is checked into the repo and embedded at compile time; \
         a parse failure means the file and this converter have drifted",
    )
}

/// A token's CSS value (`rgba(r,g,b,a)` or `#rrggbb[aa]`) as the `#RRGGBB[AA]` hex
/// string gpui-component's `try_parse_color` accepts.
pub(super) fn to_hex_color(value: &str) -> String {
    let value = value.trim();
    if let Some(digits) = value.strip_prefix('#') {
        return format!("#{}", digits.to_ascii_uppercase());
    }

    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("expected rgba()/rgb() or #hex color, got {value:?}"));

    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let channel = |s: &str| -> u32 {
        s.parse::<f32>()
            .unwrap_or_else(|_| panic!("expected a numeric color channel, got {s:?}"))
            .round() as u32
    };
    let r = channel(parts[0]);
    let g = channel(parts[1]);
    let b = channel(parts[2]);

    if let Some(alpha) = parts.get(3) {
        let a = alpha
            .parse::<f32>()
            .unwrap_or_else(|_| panic!("expected a numeric alpha channel, got {alpha:?}"));
        let a8 = (a * 255.0).round() as u32;
        format!("#{r:02X}{g:02X}{b:02X}{a8:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}")
    }
}

fn hex(value: &str) -> SharedString {
    SharedString::from(to_hex_color(value))
}

/// A `#rrggbb` token at alpha 0, as `#RRGGBB00`.
fn clear(value: &str) -> SharedString {
    let hex = to_hex_color(value);
    SharedString::from(format!("{}00", &hex[..7]))
}

fn rgb_hex(c: Rgb) -> SharedString {
    SharedString::from(format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2))
}

fn rgba_hex(c: Rgb, alpha: f64) -> SharedString {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    SharedString::from(format!("#{:02X}{:02X}{:02X}{a:02X}", c.0, c.1, c.2))
}

/// Picks the `dark`/`light` side of a `color.*` token for the mode being built.
fn side(value: &DualValue, mode: ThemeMode) -> &str {
    if mode.is_dark() {
        &value.dark
    } else {
        &value.light
    }
}

/// `color.rowSelected` for `mode`, as `#RRGGBBAA`: the link browser's selected-row
/// fill. `ThemeConfig` has no slot for it (`list_active` would also tint every other
/// `List`), so `theme::row_selected` serves it to the view.
pub(super) fn row_selected_hex(mode: ThemeMode) -> String {
    to_hex_color(side(&design_tokens().color.row_selected, mode))
}

/// Builds the `ThemeConfig` for `mode` from the tokens and the accent roles. See
/// `desktop-gpui/src/theme/mod.rs`'s module doc for the field-by-field mapping.
pub(super) fn build_theme_config(tokens: &DesignTokens, mode: ThemeMode, a: &Roles) -> ThemeConfig {
    let c = &tokens.color;
    let v = |d: &DualValue| Some(hex(side(d, mode)));
    let transparent = Some(SharedString::from("#00000000"));
    let white = Some(SharedString::from("#FFFFFF"));

    // `ThemeConfigColors` keeps a few base-palette fields private, so it cannot be
    // built with struct-literal syntax from outside gpui-component: set the public
    // fields on the default.
    let mut colors = ThemeConfigColors::default();
    // Surfaces and text. `background` is windowBackgroundColor at alpha 0:
    // gpui-component's Root paints the whole window in it, which would cover the
    // panel's glass; each view paints the card itself, opaque
    // (`theming::surface`), from the same RGB.
    colors.background = Some(clear(side(&c.window_background, mode)));
    colors.popover = v(&c.window_background);
    colors.foreground = v(&c.label);
    colors.popover_foreground = v(&c.label);
    colors.muted = v(&c.under_page_background);
    colors.muted_foreground = v(&c.secondary_label);
    colors.border = v(&c.separator);
    colors.table_row_border = v(&c.separator);
    colors.scrollbar_thumb = v(&c.tertiary_label);
    // Controls: a text field's edge, push buttons (gpui-component's `secondary`),
    // hover surfaces.
    colors.input = v(&c.control_border);
    colors.secondary = v(&c.control);
    colors.secondary_hover = v(&c.control_hover);
    colors.secondary_active = v(&c.control_active);
    colors.secondary_foreground = v(&c.label);
    colors.accent = v(&c.row_hover);
    colors.accent_foreground = v(&c.label);
    colors.list_hover = v(&c.row_hover);
    // A selected row's fill is drawn by the view (theming::selection): gpui-component
    // caps list_active at 20% alpha, too faint for macOS's selection.
    colors.list_active = transparent.clone();
    colors.list_active_border = transparent;
    colors.switch = v(&c.switch_off);
    // White (`color.switchThumb`), or black where white is under 3:1 on the checked
    // track (a light accent; `accent::derive`).
    colors.switch_thumb = if a.thumb == Rgb::WHITE {
        v(&c.switch_thumb)
    } else {
        Some(rgb_hex(a.thumb))
    };
    // The accent (theme/accent.rs): a checked switch and the caret take `primary`.
    colors.primary = Some(rgb_hex(a.mark));
    colors.primary_foreground = Some(rgb_hex(a.button_text));
    colors.primary_hover = Some(rgb_hex(a.button_hover));
    colors.primary_active = Some(rgb_hex(a.button_active));
    colors.button_primary = Some(rgb_hex(a.button));
    colors.button_primary_foreground = Some(rgb_hex(a.button_text));
    colors.button_primary_hover = Some(rgb_hex(a.button_hover));
    colors.button_primary_active = Some(rgb_hex(a.button_active));
    colors.ring = Some(rgba_hex(a.ring.0, a.ring.1));
    colors.selection = Some(rgba_hex(a.text_selection.0, a.text_selection.1));
    colors.caret = Some(rgb_hex(a.mark));
    colors.link = Some(rgb_hex(a.mark));
    // Status: text colours, and the filled destructive button (white text).
    colors.danger = v(&c.danger);
    colors.danger_foreground = white.clone();
    colors.button_danger = v(&c.danger_fill);
    // Explicit, or gpui-component derives a pale tint that white text fails on:
    // darker steps, so white keeps (more than) 4.5:1.
    colors.button_danger_hover = v(&c.danger_fill_hover);
    colors.button_danger_active = v(&c.danger_fill_active);
    colors.button_danger_foreground = white;
    colors.success = v(&c.success);
    colors.warning = v(&c.warning);

    ThemeConfig {
        name: SharedString::from(if mode.is_dark() {
            "Ubilink Dark"
        } else {
            "Ubilink Light"
        }),
        mode,
        font_size: Some(tokens.font_size.body),
        font_family: Some(SharedString::from(tokens.font.ui.clone())),
        radius: Some(tokens.radius.control.round() as usize),
        radius_lg: Some(tokens.radius.card.round() as usize),
        colors,
        ..Default::default()
    }
}

/// `font.mono` is a CSS font stack; the first *quoted* family is the first real one.
/// Read only by `tokens::tests` (pins `tokens::MONO_FONT_FAMILY`).
#[cfg(test)]
pub(super) fn first_real_font_family(stack: &str) -> String {
    stack
        .split(',')
        .map(str::trim)
        .find_map(|entry| entry.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or_else(|| panic!("expected a quoted font family in font stack {stack:?}"))
        .to_string()
}

/// The light theme for accent roles `roles` (`accent::derive(.., Light)`).
pub fn light_theme_config(roles: &Roles) -> ThemeConfig {
    build_theme_config(&design_tokens(), ThemeMode::Light, roles)
}

/// The dark theme for accent roles `roles` (`accent::derive(.., Dark)`).
pub fn dark_theme_config(roles: &Roles) -> ThemeConfig {
    build_theme_config(&design_tokens(), ThemeMode::Dark, roles)
}
