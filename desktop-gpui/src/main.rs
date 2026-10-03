// Satisfies: R2 (GPUI Phase 0 foundation). See
// docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md.
//
// The GPUI shell that replaces desktop/ (Tauri) in Phase 2; until then it ships beside
// it as `ubilink-next` (spec D4). A separate crate for the same reason desktop/ is
// (D11): the GUI tree stays out of ubilink-core, which holds all logic and is tested
// headlessly. This Phase 0 binary only proves the pinned stack builds, bundles and runs
// as a menu-bar (accessory) app; the tray, hotkeys and pop-up panel arrive in later
// tasks. Building needs full Xcode (GPUI compiles Metal shaders); running needs a
// desktop session.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        // GPUI's applicationDidFinishLaunching forces the Regular policy (Dock icon,
        // app menu) just before it calls this closure, overriding LSUIElement in the
        // bundle's Info.plist. A menu-bar utility must not sit in the Dock or take the
        // menu bar, so switch back to Accessory here.
        let mtm = MainThreadMarker::new().expect("GPUI runs this closure on the main thread");
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        // No window yet: GPUI keeps the app running with none open, so it idles until
        // quit. Task 4 adds the tray icon and the panel.
    });
}
