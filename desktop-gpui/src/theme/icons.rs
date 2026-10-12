//! The app's icon source: gpui-kit's default component bundle
//! (`gpui_kit::assets::Assets`) plus the one glyph a view needs that the bundle
//! leaves out. The full catalog (`AllAssets`, about 1,800 SVGs) would add to the
//! stripped binary's 20 MB budget (docs/PERFORMANCE.md) for one icon.
//!
//! [`LINK`] is Lucide's `link` icon, copied verbatim from gpui-kit-assets 0.7.0's
//! `assets/icons/link.svg` (Lucide, ISC License, Copyright (c) 2026 Lucide Icons and
//! Contributors; the same icon set and licence as the bundle). Nothing is fetched.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

/// The capture view's leading glyph (D26 amendment, Things-style): a neutral link,
/// since the capture carries no source-app icon.
pub const LINK: &str = "icons/ubilink-link.svg";

const LINK_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" /></svg>"#;

/// gpui-kit's default bundle, plus [`LINK`].
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == LINK {
            return Ok(Some(Cow::Borrowed(LINK_SVG.as_bytes())));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        if LINK.starts_with(path) {
            all.push(LINK.into());
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::{Assets, LINK};
    use gpui_kit::AssetSource;

    /// The extra glyph loads, and the bundle's own icons still do.
    #[test]
    fn serves_the_link_glyph_and_the_bundle() {
        let link = Assets.load(LINK).unwrap().unwrap();
        assert!(std::str::from_utf8(&link).unwrap().starts_with("<svg"));
        assert!(Assets.load("icons/inbox.svg").unwrap().is_some());
        assert!(Assets.list("icons/").unwrap().iter().any(|p| p == LINK));
    }
}
