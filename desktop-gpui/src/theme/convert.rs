// Satisfies: R12 (GPUI Phase 0 — ubilink's design tokens become gpui-component
// ThemeConfigs). See desktop-gpui/src/theme/mod.rs for the mapping table this
// module implements.
//
// `design/ubilink-tokens.json` is embedded at compile time (`include_str!`)
// rather than read at start-up: Task 12's brief requires the shell to have no
// runtime file dependency on `design/` (Phase 0's binding constraint was not to
// touch `design/` or the Tauri web panel that also rendered it).

use gpui_kit::component::{ThemeConfig, ThemeConfigColors, ThemeMode};
use gpui_kit::SharedString;
use serde::Deserialize;

/// The source of truth (`design/ubilink-tokens.json`, which the deleted Tauri web
/// panel also rendered as `dist/tokens.css`), embedded so the GPUI binary ships with
/// no runtime dependency on `design/`.
const RAW_TOKENS: &str = include_str!("../../../design/ubilink-tokens.json");

/// Mirrors `design/ubilink-tokens.json`'s shape. Only the fields Task 12 (and
/// the `tokens` module's constants) consume are typed; `$meta`, `directions`
/// and `sourceIcons` are Phase 1+ concerns (direction variants, icon keys) and
/// are left for the task that reads them, per spec D-scope (don't speculate
/// ahead of the view that needs it).
#[derive(Debug, Deserialize)]
pub(super) struct DesignTokens {
    pub(super) font: FontTokens,
    #[serde(rename = "fontSize")]
    pub(super) font_size: FontSizeTokens,
    pub(super) radius: RadiusTokens,
    // Read only by `tokens::tests::constants_match_the_embedded_json`, which
    // pins `tokens::spacing` against this field so the two cannot drift.
    // `desktop-gpui` is a `[[bin]]`-only crate, so nothing outside a test
    // build ever reads it — there is no lib target for which "pub" would
    // mean "used elsewhere".
    #[allow(dead_code)]
    pub(super) spacing: SpacingTokens,
    pub(super) status: StatusTokens,
    pub(super) color: ColorTokens,
}

#[derive(Debug, Deserialize)]
pub(super) struct FontTokens {
    pub(super) ui: String,
    /// Read only by `tokens::tests` (pins `tokens::MONO_FONT_FAMILY`); see
    /// that module's doc for why `ThemeConfig::mono_font_family` isn't set
    /// from it.
    #[allow(dead_code)]
    pub(super) mono: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct FontSizeTokens {
    // `display`/`title`/`row`/`meta`: read only by
    // `tokens::tests::constants_match_the_embedded_json`, which pins
    // `tokens::font_size::*` against them; no `ThemeConfig` slot takes them
    // (see this module's mapping table doc). `body` is the one Task 12 maps.
    #[allow(dead_code)]
    pub(super) display: f32,
    #[allow(dead_code)]
    pub(super) title: f32,
    #[allow(dead_code)]
    pub(super) row: f32,
    pub(super) body: f32,
    #[allow(dead_code)]
    pub(super) meta: f32,
}

#[derive(Debug, Deserialize)]
pub(super) struct RadiusTokens {
    pub(super) panel: f32,
    // `card`/`row`/`tile`/`kbd`/`pill`: read only by
    // `tokens::tests::constants_match_the_embedded_json`, which pins
    // `tokens::radius::*` against them; `control`/`panel` are the two Task 12
    // maps (see this module's mapping table doc).
    #[allow(dead_code)]
    pub(super) card: f32,
    #[allow(dead_code)]
    pub(super) row: f32,
    #[allow(dead_code)]
    pub(super) tile: f32,
    pub(super) control: f32,
    #[allow(dead_code)]
    pub(super) kbd: f32,
    #[allow(dead_code)]
    pub(super) pill: f32,
}

/// Read only by `tokens::tests::constants_match_the_embedded_json`, which
/// pins `tokens::spacing::*` against every field here — `ThemeConfig` has no
/// spacing slot at all (see `theme::mod`'s doc).
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

#[derive(Debug, Deserialize)]
pub(super) struct StatusTokens {
    pub(super) green: String,
    pub(super) danger: String,
}

/// One `color.*` entry: a value per theme mode, as `design/ubilink-tokens.json`
/// stores it (`{"dark": "...", "light": "..."}`). Most values are `rgba(...)`
/// or `#rrggbb` color strings; `uiShadow` is the one exception (a CSS
/// `box-shadow` string) and is handled as an opaque string for that reason.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct DualValue {
    pub(super) dark: String,
    pub(super) light: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ColorTokens {
    #[serde(rename = "uiBg")]
    pub(super) ui_bg: DualValue,
    #[serde(rename = "uiBg2")]
    pub(super) ui_bg_2: DualValue,
    #[serde(rename = "uiBorder")]
    pub(super) ui_border: DualValue,
    #[serde(rename = "uiHairline")]
    pub(super) ui_hairline: DualValue,
    // `ui_shadow`/`text3`/`kbd_bg`/`kbd_border`/`kbd_text`: intentionally
    // unmapped (see `theme::mod`'s mapping table doc) — no `ThemeConfigColors`
    // field fits them. Read only by `tokens::tests`, which pins
    // `tokens::unmapped_colors::*` against these.
    #[serde(rename = "uiShadow")]
    #[allow(dead_code)]
    pub(super) ui_shadow: DualValue,
    pub(super) text1: DualValue,
    pub(super) text2: DualValue,
    #[allow(dead_code)]
    pub(super) text3: DualValue,
    pub(super) accent: DualValue,
    #[serde(rename = "accentSoft")]
    pub(super) accent_soft: DualValue,
    #[serde(rename = "rowSelected")]
    pub(super) row_selected: DualValue,
    #[serde(rename = "rowHover")]
    pub(super) row_hover: DualValue,
    #[serde(rename = "ctrlBg")]
    pub(super) ctrl_bg: DualValue,
    #[serde(rename = "ctrlBgHover")]
    pub(super) ctrl_bg_hover: DualValue,
    #[serde(rename = "ctrlBorder")]
    pub(super) ctrl_border: DualValue,
    #[serde(rename = "kbdBg")]
    #[allow(dead_code)]
    pub(super) kbd_bg: DualValue,
    #[serde(rename = "kbdBorder")]
    #[allow(dead_code)]
    pub(super) kbd_border: DualValue,
    #[serde(rename = "kbdText")]
    #[allow(dead_code)]
    pub(super) kbd_text: DualValue,
}

/// Parses the embedded tokens once per call; cheap enough (a few hundred
/// bytes of JSON) that callers don't need a `Lazy`/`OnceLock` wrapper, and a
/// plain function keeps the parse visible to `cargo test`'s RED/GREEN cycle
/// (a `Lazy` that panicked on bad JSON would panic at first *use*, anywhere,
/// not at the call site that is actually wrong).
pub(super) fn design_tokens() -> DesignTokens {
    serde_json::from_str(RAW_TOKENS).expect(
        "design/ubilink-tokens.json is checked into the repo and embedded at compile time; \
         a parse failure means the file and this converter have drifted",
    )
}

/// Converts a `color.*` token's CSS value (`rgba(r,g,b,a)` or `#rrggbb[aa]`)
/// into the `#RRGGBBAA`/`#RRGGBB` hex string gpui-component's
/// `try_parse_color` accepts (gpui-component's `ThemeConfigColors` fields are
/// plain strings parsed with that function; it understands hex and Tailwind
/// names, not CSS `rgba(...)`, so this is where that gap gets closed).
pub(super) fn to_hex_color(value: &str) -> String {
    let value = value.trim();
    if let Some(digits) = value.strip_prefix('#') {
        // Normalize to the uppercase form gpui-component's own `Hsla::to_hex`
        // produces, so a token already given as hex (`color.accent`) and one
        // converted from `rgba(...)` here read identically.
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

/// Picks the `dark`/`light` side of a `color.*` token for the mode being
/// built.
fn side(value: &DualValue, mode: ThemeMode) -> &str {
    if mode.is_dark() {
        &value.dark
    } else {
        &value.light
    }
}

/// Builds the `ThemeConfig` for `mode` from the parsed design tokens. See
/// `desktop-gpui/src/theme/mod.rs`'s module doc for the field-by-field
/// mapping table this follows.
pub(super) fn build_theme_config(tokens: &DesignTokens, mode: ThemeMode) -> ThemeConfig {
    let c = &tokens.color;
    let accent = hex(side(&c.accent, mode));

    // macOS system blue (`color.accent`) is both the generic hover/accent tint
    // and the primary-action color in Ubilink's HUD — there is one accent in
    // the source tokens, not two — so it feeds both `accent.background` and
    // `primary.background`. Native macOS controls pair that blue with white
    // text; the tokens don't define a primary-foreground, so this is a fixed
    // choice, not a token mapping.
    let primary_foreground = SharedString::from("#FFFFFF");

    // `ThemeConfigColors` keeps a handful of base-palette fields (`blue`,
    // `red`, ...) private, so it cannot be built with struct-literal syntax
    // from outside gpui-component (even `..Default::default()` needs every
    // field nameable). Build the default, then set only the public fields
    // Task 12 maps.
    let mut colors = ThemeConfigColors::default();
    colors.background = Some(hex(side(&c.ui_bg, mode)));
    colors.foreground = Some(hex(side(&c.text1, mode)));
    colors.muted_foreground = Some(hex(side(&c.text2, mode)));
    colors.border = Some(hex(side(&c.ui_border, mode)));
    colors.table_row_border = Some(hex(side(&c.ui_hairline, mode)));
    colors.accent = Some(accent.clone());
    colors.accent_foreground = Some(hex(side(&c.text1, mode)));
    colors.primary = Some(accent);
    colors.primary_foreground = Some(primary_foreground);
    colors.selection = Some(hex(side(&c.accent_soft, mode)));
    colors.list_active = Some(hex(side(&c.row_selected, mode)));
    colors.list_hover = Some(hex(side(&c.row_hover, mode)));
    colors.secondary = Some(hex(side(&c.ctrl_bg, mode)));
    colors.secondary_hover = Some(hex(side(&c.ctrl_bg_hover, mode)));
    colors.muted = Some(hex(side(&c.ctrl_bg, mode)));
    colors.input = Some(hex(side(&c.ctrl_border, mode)));
    colors.popover = Some(hex(side(&c.ui_bg_2, mode)));
    colors.success = Some(SharedString::from(tokens.status.green.clone()));
    colors.danger = Some(SharedString::from(tokens.status.danger.clone()));

    ThemeConfig {
        name: SharedString::from(if mode.is_dark() {
            "Ubilink Dark"
        } else {
            "Ubilink Light"
        }),
        mode,
        font_size: Some(tokens.font_size.body),
        font_family: Some(SharedString::from(first_real_font_family(&tokens.font.ui))),
        radius: Some(tokens.radius.control.round() as usize),
        radius_lg: Some(tokens.radius.panel.round() as usize),
        colors,
        ..Default::default()
    }
}

/// `font.ui` is a CSS font stack: `-apple-system, "SF Pro Text", ...`. The
/// first entry is a CSS pseudo-keyword (an alias the browser resolves, not an
/// installed font name), so it names nothing GPUI's font loader could look
/// up; the first *quoted* family is the first real, nameable font.
pub(super) fn first_real_font_family(stack: &str) -> String {
    stack
        .split(',')
        .map(str::trim)
        .find_map(|entry| entry.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or_else(|| panic!("expected a quoted font family in font stack {stack:?}"))
        .to_string()
}

pub fn light_theme_config() -> ThemeConfig {
    build_theme_config(&design_tokens(), ThemeMode::Light)
}

pub fn dark_theme_config() -> ThemeConfig {
    build_theme_config(&design_tokens(), ThemeMode::Dark)
}
