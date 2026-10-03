//! The AppKit reads GPUI does not wrap, via objc2-app-kit: the displays in global
//! points (GPUI's `PlatformDisplay::bounds` on macOS is display-LOCAL, origin always 0,
//! so it cannot say which display the cursor is on), the cursor (GPUI's
//! `mouse_position` is window-relative and there is no window yet), the activation
//! policy, and a read-back of the open panel's NSWindow so `ShellState` reports what
//! AppKit actually has, not what we asked GPUI for (R4's probe).
//!
//! Everything here needs the main thread, proven by a `MainThreadMarker`.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSScreen, NSWindowStyleMask,
};
use objc2_foundation::NSString;

use super::placement::{cursor_from_appkit, Display, Rect};

/// NSPopUpWindowLevel (kCGPopUpMenuWindowLevel), the level GPUI gives `WindowKind::PopUp`.
const POPUP_WINDOW_LEVEL: isize = 101;

/// Every display in global top-left points, primary first (AppKit lists the menu-bar
/// screen first), plus the primary's height for the cursor flip.
pub fn displays(mtm: MainThreadMarker) -> (Vec<Display>, f64) {
    let screens = NSScreen::screens(mtm);
    let primary_height = screens.firstObject().map_or(0.0, |s| s.frame().size.height);
    let out = (0..screens.count())
        .filter_map(|i| {
            let s = screens.objectAtIndex(i);
            let f = s.frame();
            let v = s.visibleFrame();
            Some(Display {
                id: screen_number(&s)?,
                frame: Rect::from_appkit(
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                    primary_height,
                ),
                work: Rect::from_appkit(
                    v.origin.x,
                    v.origin.y,
                    v.size.width,
                    v.size.height,
                    primary_height,
                ),
            })
        })
        .collect();
    (out, primary_height)
}

/// `deviceDescription[@"NSScreenNumber"]` — the CGDirectDisplayID GPUI's `DisplayId`
/// wraps. Sent as raw messages because the typed accessor needs objc2-app-kit's
/// NSGraphics + objc2-foundation's NSDictionary/NSValue features, which would re-resolve
/// the lockfile; the selectors are stable AppKit API since 10.0.
fn screen_number(screen: &NSScreen) -> Option<u32> {
    let key = NSString::from_str("NSScreenNumber");
    // SAFETY: deviceDescription returns a non-nil NSDictionary; objectForKey: returns
    // nil or an NSNumber for this key; unsignedIntValue is NSNumber's.
    unsafe {
        let desc: Retained<AnyObject> = msg_send![screen, deviceDescription];
        let num: Option<Retained<AnyObject>> = msg_send![&*desc, objectForKey: &*key];
        let num = num?;
        Some(msg_send![&*num, unsignedIntValue])
    }
}

/// The cursor in global top-left points.
pub fn cursor(primary_height: f64) -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    cursor_from_appkit(p.x, p.y, primary_height)
}

pub fn activation_policy(mtm: MainThreadMarker) -> NSApplicationActivationPolicy {
    NSApplication::sharedApplication(mtm).activationPolicy()
}

/// What AppKit reports for the visible ubilink panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelFacts {
    /// An NSPanel with the non-activating style at NSPopUpWindowLevel: GPUI's PopUp.
    pub popup: bool,
    /// An NSVisualEffectView sits in the content view: GPUI's Blurred background.
    pub blurred: bool,
}

/// Inspect the app's visible GPUI panel window, if any. The shell only ever has one
/// panel, so the first visible NSPanel is it.
pub fn inspect_panel(mtm: MainThreadMarker) -> Option<PanelFacts> {
    let panel_class = AnyClass::get(c"NSPanel")?;
    let effect_class = AnyClass::get(c"NSVisualEffectView")?;
    let windows = NSApplication::sharedApplication(mtm).windows();
    (0..windows.count())
        .map(|i| windows.objectAtIndex(i))
        .find(|w| w.isVisible() && w.isKindOfClass(panel_class))
        .map(|w| {
            let popup = w.level() == POPUP_WINDOW_LEVEL
                && w.styleMask()
                    .contains(NSWindowStyleMask::NonactivatingPanel);
            let blurred = w.contentView().is_some_and(|v| {
                let subs = v.subviews();
                (0..subs.count()).any(|i| subs.objectAtIndex(i).isKindOfClass(effect_class))
            });
            PanelFacts { popup, blurred }
        })
}
