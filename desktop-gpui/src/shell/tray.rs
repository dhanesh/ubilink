//! The menu-bar item (R4): with no Dock icon (accessory policy) it is the app's
//! always-reachable home, as it was in the Tauri shell. GPUI has no status-item API, so this
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
        &MenuItem::with_id(ID_QUIT, "Quit Ubilink", true, None),
    ])
    .map_err(|e| format!("tray menu: {e}"))?;
    let icon = Icon::from_rgba(link_glyph(ICON_PX), ICON_PX, ICON_PX)
        .map_err(|e| format!("tray icon: {e}"))?;
    TrayIconBuilder::new()
        .with_id("ubilink-tray")
        .with_tooltip("Ubilink — ⌘⇧K to capture a link")
        .with_menu(Box::new(menu))
        // Template image: macOS recolours it for light/dark menu bars, as it did for
        // the deleted Tauri shell's ubilink-link.png.
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

/// The app icon's node-link glyph (two filled nodes joined by a line) as
/// black-on-transparent RGBA, so the menu-bar icon matches the app icon. Geometry is
/// the deleted Tauri shell's `desktop/icons/ubilink-link.svg` (ee80e70), in its 44-unit
/// box: nodes r 4.4 at (12, 32) and (32, 12), a round-capped 4.6-wide line from
/// (15, 29) to (29, 15). Drawn in code: decoding that PNG would need an image decoder,
/// a new external crate.
fn link_glyph(size: u32) -> Vec<u8> {
    let k = f64::from(size) / 44.0;
    let nodes = [(12.0 * k, 32.0 * k), (32.0 * k, 12.0 * k)];
    let (node_r, half_line) = (4.4 * k, 2.3 * k);
    let (a, b) = ((15.0 * k, 29.0 * k), (29.0 * k, 15.0 * k));
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let p = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            // Coverage = how far inside a shape the pixel centre is, so every edge is
            // anti-aliased over one pixel.
            let node = nodes
                .iter()
                .map(|&c| node_r - dist(p, c))
                .fold(f64::MIN, f64::max);
            let line = half_line - dist_to_segment(p, a, b);
            let cov = (node.max(line) + 0.5).clamp(0.0, 1.0);
            let i = ((y * size + x) * 4) as usize;
            rgba[i + 3] = (cov * 255.0) as u8;
        }
    }
    rgba
}

fn dist(p: (f64, f64), q: (f64, f64)) -> f64 {
    ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt()
}

/// Distance from `p` to the segment `a`–`b` (round caps fall out of the clamp).
fn dist_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    dist(p, (a.0 + t * dx, a.1 + t * dy))
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

    /// The app icon's shape: solid nodes and the joining line, empty corners. The old
    /// chain-link rings were hollow, so a node centre being opaque tells them apart.
    #[test]
    fn glyph_is_the_app_icons_node_link() {
        let g = link_glyph(ICON_PX);
        assert_eq!(g.len(), (ICON_PX * ICON_PX * 4) as usize);
        let alpha = |x: u32, y: u32| g[((y * ICON_PX + x) * 4 + 3) as usize];
        assert_eq!(alpha(12, 32), 255, "lower-left node is solid");
        assert_eq!(alpha(32, 12), 255, "upper-right node is solid");
        assert_eq!(alpha(21, 22), 255, "the line joins them");
        for (x, y) in [(0, 0), (43, 0), (0, 43), (43, 43), (12, 12), (32, 32)] {
            assert_eq!(alpha(x, y), 0, "({x}, {y}) is off the glyph");
        }
    }
}
