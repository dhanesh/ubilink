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
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSScreen, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::placement::{cursor_from_appkit, Display, Rect};

/// NSPopUpWindowLevel (kCGPopUpMenuWindowLevel), the level GPUI gives `WindowKind::PopUp`.
const POPUP_WINDOW_LEVEL: isize = 101;
/// NSNormalWindowLevel: where a file dialog opens.
const NORMAL_WINDOW_LEVEL: isize = 0;

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

/// The app's visible GPUI panel window, if any. The shell has one panel at a time,
/// but GPUI closes a removed window's NSWindow on a later run-loop turn, so for a
/// moment after a hide or route reopen the old panel is still visible beside the new
/// one (Task 6 saw this live: `window_id` right after `resign_key` found the closing
/// panel). NSApp lists windows oldest first, so the newest visible NSPanel is the
/// current one; callers that need "is a panel open at all" ask the shell first.
///
/// File dialogs (NSSavePanel, and NSOpenPanel below it) are NSPanels too; they are
/// never ours, so they are skipped even while one is newer and on screen.
fn visible_panel(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let panel_class = AnyClass::get(c"NSPanel")?;
    let dialog_class = AnyClass::get(c"NSSavePanel");
    let windows = NSApplication::sharedApplication(mtm).windows();
    (0..windows.count())
        .rev()
        .map(|i| windows.objectAtIndex(i))
        .find(|w| {
            w.isVisible()
                && w.isKindOfClass(panel_class)
                && !dialog_class.is_some_and(|c| w.isKindOfClass(c))
        })
}

/// Settings' Import (release builds): before a file dialog opens, drop the panel to
/// the normal window level, so the dialog (ordered front after it, at the normal
/// level) is not hidden under a pop-up-level panel. Returns whether there was a panel.
pub fn lower_panel_for_dialog(mtm: MainThreadMarker) -> bool {
    visible_panel(mtm)
        .map(|w| w.setLevel(NORMAL_WINDOW_LEVEL))
        .is_some()
}

/// The dialog closed (chosen, cancelled or failed): the panel goes back to its
/// pop-up level and takes key status again, so Tab and Escape reach Settings. Call
/// outside any GPUI update: becoming key notifies GPUI synchronously.
pub fn restore_panel_after_dialog(mtm: MainThreadMarker) {
    if let Some(w) = visible_panel(mtm) {
        w.setLevel(POPUP_WINDOW_LEVEL);
        w.makeKeyWindow();
    }
}

/// Inspect the app's visible GPUI panel window, if any.
pub fn inspect_panel(mtm: MainThreadMarker) -> Option<PanelFacts> {
    let effect_class = AnyClass::get(c"NSVisualEffectView")?;
    visible_panel(mtm).map(|w| {
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

/// Move and resize the visible panel to `size` points with its top-left corner at
/// `top_left` (global top-left points, as `placement`), flipping to AppKit's
/// bottom-left frame with the primary display's height. Returns whether there was a
/// panel. GPUI hears of the new size through the window's resize notification and
/// lays the view out again. Call outside any GPUI update (see `shell::fit_panel`).
pub fn set_panel_frame(
    mtm: MainThreadMarker,
    top_left: (f64, f64),
    size: (f64, f64),
    primary_height: f64,
) -> bool {
    let Some(w) = visible_panel(mtm) else {
        return false;
    };
    let (x, y) = top_left;
    let (width, height) = size;
    let frame = NSRect::new(
        NSPoint::new(x, primary_height - (y + height)),
        NSSize::new(width, height),
    );
    w.setFrame_display(frame, true);
    true
}

/// Automation `window_id` (R6): the panel's CGWindowID (`windowNumber`), what
/// `screencapture -l` takes.
#[cfg(feature = "automation")]
pub fn panel_window_number(mtm: MainThreadMarker) -> Option<u32> {
    let n = visible_panel(mtm)?.windowNumber();
    u32::try_from(n).ok().filter(|n| *n > 0)
}

/// Automation `a11y` (D6, TN5): ask the panel's content view for its accessibility
/// children, as an assistive client would. GPUI installed accesskit_macos's
/// subclassing adapter on exactly this view (`SubclassingAdapter::for_window` takes the
/// window's contentView), and its `accessibilityChildren` override is what activates
/// the adapter and GPUI's tree. Returns whether there was a view to ask.
#[cfg(feature = "automation")]
pub fn poke_accessibility(mtm: MainThreadMarker) -> bool {
    let Some(view) = visible_panel(mtm).and_then(|w| w.contentView()) else {
        return false;
    };
    // SAFETY: accessibilityChildren is NSAccessibility API on every NSView, takes no
    // arguments and returns nil or an autoreleased NSArray, which `Retained` balances.
    let _children: Option<Retained<AnyObject>> =
        unsafe { msg_send![&*view, accessibilityChildren] };
    true
}

/// Automation `set_appearance` (R6): set NSApp's appearance by name
/// ("NSAppearanceNameAqua", "NSAppearanceNameDarkAqua"), or clear it (`None`) to
/// follow the system again. Raw messages for the same reason as `screen_number`: the
/// typed API needs objc2-app-kit's NSAppearance feature, which Cargo.toml does not
/// enable.
#[cfg(feature = "automation")]
pub fn set_app_appearance(mtm: MainThreadMarker, name: Option<&str>) {
    let app = NSApplication::sharedApplication(mtm);
    let appearance: Option<Retained<AnyObject>> = name.and_then(|n| {
        let class = AnyClass::get(c"NSAppearance")?;
        let n = NSString::from_str(n);
        // SAFETY: +[NSAppearance appearanceNamed:] takes an NSString and returns nil
        // or an autoreleased NSAppearance.
        unsafe { msg_send![class, appearanceNamed: &*n] }
    });
    // SAFETY: -[NSApplication setAppearance:] takes nil or an NSAppearance, on the
    // main thread (proven by `mtm`).
    unsafe {
        let _: () = msg_send![&*app, setAppearance: appearance.as_deref()];
    }
}
