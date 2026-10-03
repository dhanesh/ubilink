// Satisfies: R2 (GPUI Phase 0 foundation). See
// docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md.
//
// The GPUI shell that replaces desktop/ (Tauri) in Phase 2; until then it ships beside
// it as `ubilink-next` (spec D4). A separate crate for the same reason desktop/ is
// (D11): the GUI tree stays out of ubilink-core, which holds all logic and is tested
// headlessly. This Phase 0 binary only proves the pinned stack builds, bundles and runs
// as a menu-bar (accessory) app with the shell of R4 (tray, hotkeys, pop-up panel —
// see src/shell/). Building needs full Xcode (GPUI compiles Metal shaders); running needs a
// desktop session.

mod shell;
mod theme;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use shell::single_instance::{self, Claim};
use shell::{Command, Route, ShellEvent};

fn main() {
    // Single instance (R4's "one process"): settle it before GPUI starts, so a second
    // launch exits without ever creating windows, a tray item or hotkeys.
    // The listener thread posts to the shell's inbox, created here so it exists first.
    let (inbox, events) = shell::inbox();
    let on_show = inbox.clone();
    match single_instance::claim(move || on_show.post(ShellEvent::Show)) {
        Claim::Signalled => {
            eprintln!("[ubilink-next] already running: asked it to show the panel");
            return;
        }
        Claim::Primary => {}
        Claim::Unguarded(e) => eprintln!("[ubilink-next] single-instance guard unavailable: {e}"),
    }

    let app = gpui_kit::application();
    // `open ubilink-next.app` while it runs: LaunchServices sends a reopen instead of
    // starting a second process. Show the panel, as the Tauri shell's tray "Show" did.
    app.on_reopen(|cx| shell::execute(Command::Open(Route::Capture), cx));
    app.run(move |cx| {
        gpui_kit::init(cx);
        // GPUI's applicationDidFinishLaunching forces the Regular policy (Dock icon,
        // app menu) just before it calls this closure, overriding LSUIElement in the
        // bundle's Info.plist. A menu-bar utility must not sit in the Dock or take the
        // menu bar, so switch back to Accessory here.
        let mtm = MainThreadMarker::new().expect("GPUI runs this closure on the main thread");
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        // Satisfies R12: replace gpui-component's default light/dark themes
        // with ubilink's own (design/ubilink-tokens.json), matching whichever
        // mode the system is in right now. See desktop-gpui/src/theme/mod.rs.
        theme::install(cx);
        // TN4: tray-icon and global-hotkey need the main thread and a running NSApp
        // loop, which exist only from here on.
        shell::start(cx, inbox, events);
    });
}
