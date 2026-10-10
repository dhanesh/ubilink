//! The menu-bar item (R4): with no Dock icon (accessory policy) it is the app's
//! always-reachable home, as in the Tauri shell. GPUI has no status-item API, so this
//! uses tray-icon 0.26 (an NSStatusItem underneath) with a muda menu.
//!
//! TN4: built on the main thread inside GPUI's run closure, after NSApp exists, and kept
//! alive in the `Shell` global (dropping `TrayIcon` removes the item). Menu clicks land
//! in muda's `MenuEvent` channel, drained by `shell::pump`.

use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use super::Command;
use super::Route;

pub const ID_CAPTURE: &str = "capture";
pub const ID_BROWSE: &str = "browse";
pub const ID_SETTINGS: &str = "settings";
pub const ID_QUIT: &str = "quit";

/// Map a menu item id to the shell command it triggers.
pub fn command_for(id: &str) -> Option<Command> {
    match id {
        ID_CAPTURE => Some(Command::Open(Route::Capture)),
        ID_BROWSE => Some(Command::Open(Route::Browser)),
        ID_SETTINGS => Some(Command::Open(Route::Settings)),
        ID_QUIT => Some(Command::Quit),
        _ => None,
    }
}

/// Build the tray item. Errors are returned (and recorded as tray_present=false by the
/// caller) rather than panicking: the hotkeys still work without it.
pub fn build() -> Result<TrayIcon, String> {
    // Accelerators are not attached: the chords are global hotkeys, not menu
    // shortcuts, so the label shows them instead.
    let menu = Menu::with_items(&[
        &MenuItem::with_id(ID_CAPTURE, "Capture link   ⌘⇧K", true, None),
        &MenuItem::with_id(ID_BROWSE, "Browse links   ⌘⇧L", true, None),
        &MenuItem::with_id(ID_SETTINGS, "Settings…   ⌘⇧,", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_QUIT, "Quit ubilink", true, None),
    ])
    .map_err(|e| format!("tray menu: {e}"))?;
    let icon = Icon::from_rgba(link_glyph(ICON_PX), ICON_PX, ICON_PX)
        .map_err(|e| format!("tray icon: {e}"))?;
    TrayIconBuilder::new()
        .with_id("ubilink-tray")
        .with_tooltip("Ubilink — ⌘⇧K to capture a link")
        .with_menu(Box::new(menu))
        // Template image: macOS recolours it for light/dark menu bars, as the Tauri
        // shell's ubilink-link.png does.
        .with_icon_templated(icon)
        .build()
        .map_err(|e| format!("tray: {e}"))
}

/// Whether the live NSStatusItem exists and is visible — read from the object, so a
/// status item macOS hid (menu bar overflow, or a menu-bar manager) reports false.
pub fn present(tray: &TrayIcon) -> bool {
    tray.ns_status_item().is_some_and(|item| item.isVisible())
}

/// Status-bar icon size in pixels: 22 pt rendered at 2x (tray-icon scales the image to
/// the bar height).
const ICON_PX: u32 = 44;

/// A two-ring "link" glyph as black-on-transparent RGBA, drawn in code: decoding the
/// Tauri shell's PNG would need an image decoder, a new external crate. Two rings
/// overlap on the diagonal, like a chain link.
fn link_glyph(size: u32) -> Vec<u8> {
    let s = f64::from(size);
    let (r, stroke) = (s * 0.24, s * 0.085);
    let centres = [(s * 0.37, s * 0.63), (s * 0.63, s * 0.37)];
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            // Coverage = how far inside the ring's stroke the pixel centre is, so the
            // edge is anti-aliased over one pixel.
            let cov = centres
                .iter()
                .map(|(cx, cy)| {
                    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    (stroke / 2.0 - (d - r).abs() + 0.5).clamp(0.0, 1.0)
                })
                .fold(0.0, f64::max);
            let i = ((y * size + x) * 4) as usize;
            rgba[i + 3] = (cov * 255.0) as u8;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_commands() {
        assert_eq!(command_for(ID_CAPTURE), Some(Command::Open(Route::Capture)));
        assert_eq!(command_for(ID_BROWSE), Some(Command::Open(Route::Browser)));
        assert_eq!(
            command_for(ID_SETTINGS),
            Some(Command::Open(Route::Settings))
        );
        assert_eq!(command_for(ID_QUIT), Some(Command::Quit));
        assert_eq!(command_for("nope"), None);
    }

    #[test]
    fn glyph_has_ink_and_transparent_corners() {
        let g = link_glyph(ICON_PX);
        assert_eq!(g.len(), (ICON_PX * ICON_PX * 4) as usize);
        assert_eq!(g[3], 0, "top-left corner transparent");
        assert!(
            g.chunks(4).filter(|p| p[3] > 128).count() > 50,
            "rings drawn"
        );
    }
}
