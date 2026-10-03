//! Where the panel opens: on the display under the mouse cursor, centred horizontally
//! with its top edge a fifth of the way down (Spotlight-style), and never spilling off
//! that display. Ported from desktop/src/placement.rs (the Tauri shell) so both shells
//! agree on placement (R4: "on the display under the cursor"). Pure, so the
//! multi-display cases — which need hardware to try by hand — are covered by tests.
//!
//! Everything here is in global POINTS with a TOP-LEFT origin at the primary display's
//! top-left corner (CGDisplayBounds space). AppKit reports screens and the cursor
//! bottom-left-origin instead; `Rect::from_appkit` and `cursor_from_appkit` flip them.
//! Unlike the Tauri port there is no per-monitor scale juggling: NSScreen frames and
//! NSEvent.mouseLocation are both already in points.
//!
//! GPUI takes a window's origin RELATIVE to its target display's frame (see
//! gpui-pre-macos 0.3.7 `MacWindow::open`: `screen_frame.origin + bounds.origin`), so
//! `place_panel` returns both the chosen display id and the display-local origin.

/// How far down the display the panel's top edge sits, as a fraction of height.
pub const TOP_FRACTION: f64 = 0.2;

/// A rectangle in global points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// From an AppKit rect (bottom-left origin at the primary display's bottom-left),
    /// given the primary display's height in points.
    pub fn from_appkit(x: f64, y: f64, width: f64, height: f64, primary_height: f64) -> Rect {
        Rect {
            x,
            y: primary_height - (y + height),
            width,
            height,
        }
    }

    /// Half-open, so a shared edge belongs to exactly one display.
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// The cursor in global top-left points, from AppKit's bottom-left
/// `NSEvent.mouseLocation`, given the primary display's height in points.
pub fn cursor_from_appkit(x: f64, y: f64, primary_height: f64) -> (f64, f64) {
    (x, primary_height - y)
}

/// A display: its CGDirectDisplayID (what GPUI's `DisplayId` wraps on macOS), its whole
/// frame, and the part windows may use (frame minus menu bar and Dock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub id: u32,
    pub frame: Rect,
    pub work: Rect,
}

/// Where to open a panel: which display, and the top-left corner both relative to that
/// display's frame (what GPUI's `WindowOptions::window_bounds` wants) and globally.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub display_id: u32,
    pub local: (f64, f64),
    pub global: (f64, f64),
}

/// Placement for a panel of `(width, height)` points. The display is chosen by its
/// FRAME, so a cursor up in the menu bar still counts as on that display; the panel is
/// placed within its usable area. If the cursor is on no display, the first is used
/// (the caller lists the primary display first). `None` when there are no displays:
/// let GPUI fall back to its default.
pub fn place_panel(
    displays: &[Display],
    cursor: (f64, f64),
    (width, height): (f64, f64),
) -> Option<Placement> {
    let d = displays
        .iter()
        .find(|d| d.frame.contains(cursor))
        .or_else(|| displays.first())?;
    let s = d.work;
    let x = s.x + ((s.width - width) / 2.0).max(0.0);
    // Near the top, but moved up rather than hanging off the bottom edge, and never
    // above the top edge.
    let y = (s.y + s.height * TOP_FRACTION)
        .min(s.y + s.height - height)
        .max(s.y);
    Some(Placement {
        display_id: d.id,
        local: (x - d.frame.x, y - d.frame.y),
        global: (x, y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: (f64, f64) = (640.0, 140.0);

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }
    /// A display whose usable area is its whole frame (no menu bar or Dock).
    fn display(id: u32, x: f64, y: f64, width: f64, height: f64) -> Display {
        let s = rect(x, y, width, height);
        Display {
            id,
            frame: s,
            work: s,
        }
    }

    #[test]
    fn single_screen_centres_horizontally_near_the_top() {
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let p = place_panel(&[main], (100.0, 100.0), PANEL).unwrap();
        assert_eq!(p.display_id, 1);
        assert_eq!(p.global, ((1440.0 - 640.0) / 2.0, 900.0 * TOP_FRACTION));
        assert_eq!(p.local, p.global, "primary display: local == global");
    }

    #[test]
    fn opens_on_the_screen_under_the_cursor_with_a_display_local_origin() {
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let right = display(2, 1440.0, 0.0, 1920.0, 1080.0);
        let p = place_panel(&[main, right], (2000.0, 500.0), PANEL).unwrap();
        assert_eq!(p.display_id, 2);
        assert_eq!(p.global.0, 1440.0 + (1920.0 - 640.0) / 2.0);
        // GPUI offsets window_bounds by the target display's frame origin.
        assert_eq!(p.local, ((1920.0 - 640.0) / 2.0, 1080.0 * TOP_FRACTION));
    }

    #[test]
    fn handles_screens_left_of_and_above_the_primary() {
        // Negative global coordinates are normal on macOS for screens arranged left of
        // or above the primary display.
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let left = display(3, -1920.0, -180.0, 1920.0, 1080.0);
        let p = place_panel(&[main, left], (-500.0, 0.0), PANEL).unwrap();
        assert_eq!(p.display_id, 3);
        assert_eq!(
            p.global,
            (
                -1920.0 + (1920.0 - 640.0) / 2.0,
                -180.0 + 1080.0 * TOP_FRACTION
            )
        );
        assert_eq!(p.local, ((1920.0 - 640.0) / 2.0, 1080.0 * TOP_FRACTION));
    }

    #[test]
    fn cursor_off_every_screen_falls_back_to_the_first() {
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let right = display(2, 1440.0, 200.0, 1920.0, 1080.0);
        // In the gap above the right-hand screen, which starts lower.
        let p = place_panel(&[main, right], (2000.0, 50.0), PANEL).unwrap();
        assert_eq!(p.display_id, 1);
    }

    #[test]
    fn a_screen_edge_belongs_to_one_screen_only() {
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let right = display(2, 1440.0, 0.0, 1920.0, 1080.0);
        let p = place_panel(&[main, right], (1440.0, 10.0), PANEL).unwrap();
        assert_eq!(p.display_id, 2);
    }

    #[test]
    fn no_screens_means_no_placement() {
        assert_eq!(place_panel(&[], (0.0, 0.0), PANEL), None);
    }

    #[test]
    fn never_spills_off_a_screen_smaller_than_the_panel() {
        let tiny = display(1, 100.0, 50.0, 500.0, 300.0);
        let p = place_panel(&[tiny], (200.0, 100.0), (640.0, 900.0)).unwrap();
        assert_eq!(p.global, (100.0, 50.0), "pinned to the top-left edge");
        assert_eq!(p.local, (0.0, 0.0));
    }

    #[test]
    fn a_tall_panel_moves_up_rather_than_off_the_bottom() {
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let p = place_panel(&[main], (10.0, 10.0), (640.0, 800.0)).unwrap();
        assert_eq!(p.global.1, 100.0);
    }

    #[test]
    fn places_within_the_usable_area_but_picks_the_display_by_its_frame() {
        let d = Display {
            id: 1,
            frame: rect(0.0, 0.0, 1440.0, 900.0),
            work: rect(0.0, 25.0, 1440.0, 805.0),
        };
        let p = place_panel(&[d], (700.0, 5.0), PANEL).unwrap();
        assert_eq!(
            p.global,
            ((1440.0 - 640.0) / 2.0, 25.0 + 805.0 * TOP_FRACTION)
        );
        let p = place_panel(&[d], (700.0, 5.0), (640.0, 800.0)).unwrap();
        assert_eq!(p.global.1, 25.0 + 805.0 - 800.0, "clear of the Dock");
    }

    #[test]
    fn appkit_coordinates_flip_to_top_left_global_points() {
        // Primary 1440x900 at AppKit (0,0). A 1920x1080 display ABOVE it sits at
        // AppKit y=900 (bottom-left origin grows upward) — global top-left y=-1080.
        let primary_h = 900.0;
        assert_eq!(
            Rect::from_appkit(0.0, 0.0, 1440.0, 900.0, primary_h),
            rect(0.0, 0.0, 1440.0, 900.0)
        );
        assert_eq!(
            Rect::from_appkit(0.0, 900.0, 1920.0, 1080.0, primary_h),
            rect(0.0, -1080.0, 1920.0, 1080.0)
        );
        // Usable area under a 25 pt menu bar: AppKit visibleFrame y=0, h=875.
        assert_eq!(
            Rect::from_appkit(0.0, 0.0, 1440.0, 875.0, primary_h),
            rect(0.0, 25.0, 1440.0, 875.0)
        );
        // Cursor at the AppKit top edge of the primary is global y=0.
        assert_eq!(cursor_from_appkit(10.0, 900.0, primary_h), (10.0, 0.0));
        // Cursor on the upper display lands inside its flipped frame.
        let upper = display(2, 0.0, -1080.0, 1920.0, 1080.0);
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let cursor = cursor_from_appkit(500.0, 1500.0, primary_h);
        assert_eq!(
            place_panel(&[main, upper], cursor, PANEL)
                .unwrap()
                .display_id,
            2
        );
    }
}
