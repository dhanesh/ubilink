// Satisfies: R2 (GPUI Phase 0 foundation). See
// docs/superpowers/specs/2026-10-03-gpui-phase0-foundation.md.
//
// The GPUI shell that replaced desktop/ (Tauri) in Phase 2b: it ships as `ubilink`
// (Ubilink.app, `org.dhanesh.ubilink`), the Tauri app's identity and data dir (D24). A
// separate crate for the same reason the Tauri shell was (D11): the GUI tree stays out of
// ubilink-core, which holds all logic and is tested headlessly. This Phase 0 binary only proves the pinned stack builds, bundles and runs
// as a menu-bar (accessory) app with the shell of R4 (tray, hotkeys, pop-up panel —
// see src/shell/). Building needs full Xcode (GPUI compiles Metal shaders); running needs a
// desktop session.

// R6/S1: the automation channel exists only in `automation` builds; gating the module
// declaration itself keeps every byte of it out of a release binary (TN1, checked by
// `scripts/probe.py release-clean`).
mod app_state;
#[cfg(feature = "automation")]
mod automation;
mod fixtures;
mod login_item;
mod opener;
mod paths;
mod shell;
mod theme;
mod views;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use shell::single_instance::{self, Claim};
use shell::{Command, Route, ShellEvent};

fn main() {
    // O1: a probe-named automation instance without its own data dir would use the
    // user's real one; refuse before anything is created or claimed.
    #[cfg(feature = "automation")]
    if let Err(e) = paths::instance_guard(
        std::env::var_os(automation::socket::INSTANCE_ENV).as_ref(),
        std::env::var_os(paths::DATA_DIR_ENV).as_ref(),
    ) {
        eprintln!("[ubilink] {e}");
        std::process::exit(2);
    }
    // Resolved once at startup and logged, so a run's data location is visible in its
    // log (and a relative override is reported) before any store opens it.
    eprintln!("[ubilink] data dir: {}", paths::data_dir().display());
    // Single instance (R4's "one process"): settle it before GPUI starts, so a second
    // launch exits without ever creating windows, a tray item or hotkeys.
    // The listener thread posts to the shell's inbox, created here so it exists first.
    let (inbox, events) = shell::inbox();
    let on_show = inbox.clone();
    match single_instance::claim(move || on_show.post(ShellEvent::Show)) {
        Claim::Signalled => {
            eprintln!("[ubilink] already running: asked it to show the panel");
            return;
        }
        Claim::Primary => {}
        Claim::Unguarded(e) => eprintln!("[ubilink] single-instance guard unavailable: {e}"),
    }
    // After the single-instance claim, so a second launch never binds (or clears) the
    // first one's automation socket; before GPUI, like the single-instance listener:
    // requests queue in the inbox until the pump starts.
    #[cfg(feature = "automation")]
    automation::start(inbox.clone());
    // Final review I5: the one AppState every view reads through app_state::get(),
    // built once here (primary instance only, so a second launch never opens the
    // graph) from paths::data_dir(); automation builds skip the OS permission probe.
    app_state::init();

    // gpui-kit's icon set (Lucide), so gpui-component widgets that draw an icon (a
    // Switch's tooltip, an Input's clear button, ...) find their SVGs.
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    // `open Ubilink.app` while it runs: LaunchServices sends a reopen instead of
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
