//! A source app's icon for the capture view (rc.6, D26 amendment), read locally from
//! AppKit and never from the network:
//!
//! 1. the running app whose `localizedName` is the capture's app name
//!    (`NSWorkspace.runningApplications`, `NSRunningApplication.icon`), else
//! 2. the app bundle of that name in the standard application folders
//!    (`NSWorkspace.iconForFile`), else
//! 3. none: the view keeps its neutral link glyph.
//!
//! The `NSImage` is drawn into a [`ICON_PX`]-square bitmap and encoded as PNG, which
//! GPUI decodes like any image. Results (including "none") are cached per name, so a
//! capture view asks AppKit once per app per run. Main thread only (AppKit drawing):
//! [`icon_for`] takes a [`MainThreadMarker`], and the AppKit work runs inside an
//! autorelease pool so its temporaries are released at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui_kit::{Image, ImageFormat};
use objc2::rc::{autoreleasepool, Retained};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage,
    NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// The bitmap's side in pixels: 16 pt at 2x, with headroom for a 32 pt glyph.
pub const ICON_PX: isize = 64;

/// Where app bundles live, searched when the app is not running.
const APP_FOLDERS: [&str; 4] = [
    "/Applications",
    "/System/Applications",
    "/Applications/Utilities",
    "/System/Applications/Utilities",
];

/// The app bundle paths searched for `app`, in order.
pub fn bundle_candidates(app: &str) -> Vec<String> {
    let name = app.trim();
    if name.is_empty() || name.contains('/') {
        return Vec::new();
    }
    APP_FOLDERS
        .iter()
        .map(|dir| format!("{dir}/{name}.app"))
        .collect()
}

type Cache = Mutex<HashMap<String, Option<Arc<Image>>>>;

fn cache() -> &'static Cache {
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `app`'s icon as a GPUI image, or `None` when AppKit has none for it. `_mtm` proves
/// the caller is on the main thread, where AppKit drawing belongs.
pub fn icon_for(_mtm: MainThreadMarker, app: &str) -> Option<Arc<Image>> {
    let key = app.trim().to_string();
    if key.is_empty() {
        return None;
    }
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let image = autoreleasepool(|_| ns_icon(&key).and_then(|icon| png(&icon)))
        .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
    if let Ok(mut c) = cache().lock() {
        c.insert(key, image.clone());
    }
    image
}

/// The running app's icon, else its bundle's.
fn ns_icon(app: &str) -> Option<Retained<NSImage>> {
    let workspace = NSWorkspace::sharedWorkspace();
    let running = workspace.runningApplications();
    for running_app in running.iter() {
        let named = running_app
            .localizedName()
            .is_some_and(|n| n.to_string() == app);
        if named {
            if let Some(icon) = running_app.icon() {
                return Some(icon);
            }
        }
    }
    bundle_candidates(app)
        .into_iter()
        .find(|path| std::path::Path::new(path).is_dir())
        .map(|path| workspace.iconForFile(&NSString::from_str(&path)))
}

/// `icon` drawn into an [`ICON_PX`]-square RGBA bitmap, as PNG bytes.
fn png(icon: &NSImage) -> Option<Vec<u8>> {
    // SAFETY: null planes ask AppKit to allocate the bitmap; the sizes describe a
    // valid 8-bit RGBA layout (bytes per row and bits per pixel 0: AppKit computes).
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            ICON_PX,
            ICON_PX,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    let side = ICON_PX as f64;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    icon.drawInRect(NSRect::new(NSPoint::new(0., 0.), NSSize::new(side, side)));
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: an empty properties dictionary is valid for PNG.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::bundle_candidates;

    /// The bundle search covers the standard folders and refuses a path.
    #[test]
    fn bundle_candidates_are_the_standard_folders() {
        assert_eq!(
            bundle_candidates("Safari "),
            [
                "/Applications/Safari.app",
                "/System/Applications/Safari.app",
                "/Applications/Utilities/Safari.app",
                "/System/Applications/Utilities/Safari.app",
            ]
        );
        assert!(bundle_candidates("").is_empty());
        assert!(bundle_candidates("../x").is_empty());
    }
}
