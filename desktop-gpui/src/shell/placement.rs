//! Where the panel opens: on the display under the mouse cursor, centred horizontally
//! with its top edge a fifth of the way down (Spotlight-style), and never spilling off
//! that display. Ported from the Tauri shell's desktop/src/placement.rs (deleted in
//! D24) so the GPUI app kept its placement (R4: "on the display under the cursor"). Pure, so the
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

/// Where to open a panel: which display, the top-left corner both relative to that
/// display's frame (what GPUI's `WindowOptions::window_bounds` wants) and globally,
/// and its size, the height capped to the display's usable area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub display_id: u32,
    pub local: (f64, f64),
    pub global: (f64, f64),
    pub size: (f64, f64),
}

/// Placement for a panel of `(width, height)` points. The display is chosen by its
/// FRAME, so a cursor up in the menu bar still counts as on that display; the panel is
/// placed within its usable area, and a panel taller than that area is cut to it (its
/// view scrolls inside). If the cursor is on no display, the first is used (the caller
/// lists the primary display first). `None` when there are no displays: let GPUI fall
/// back to its default.
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
    Some(fit_on(
        d,
        (x, s.y + s.height * TOP_FRACTION),
        (width, height),
    ))
}

/// The panel resized to `(width, height)` on display `d`, keeping its left edge and,
/// when it still fits, its top edge (the Tauri shell's `resize_panel`: the panel grows
/// downward with its content). Width and height are cut to the display's usable area,
/// and the panel is moved inside it: up rather than past the bottom, and back onto the
/// display when its old place is elsewhere (the display it was on went away).
pub fn fit_on(d: &Display, (x, top): (f64, f64), (width, height): (f64, f64)) -> Placement {
    let s = d.work;
    let width = width.min(s.width).max(0.0);
    let height = height.min(s.height).max(0.0);
    let x = x.min(s.x + s.width - width).max(s.x);
    let y = top.min(s.y + s.height - height).max(s.y);
    Placement {
        display_id: d.id,
        local: (x - d.frame.x, y - d.frame.y),
        global: (x, y),
        size: (width, height),
    }
}

/// AppKit's bottom-left frame origin for a rect at `top_left` (global top-left
/// points) of `size`, given the primary display's height: the inverse of
/// [`Rect::from_appkit`].
pub fn appkit_origin(top_left: (f64, f64), size: (f64, f64), primary_height: f64) -> (f64, f64) {
    (top_left.0, primary_height - (top_left.1 + size.1))
}

/// [`fit_on`] the display with id `display_id`, else the first (a display that was
/// unplugged while the panel was open). `None` when there are no displays.
pub fn refit(
    displays: &[Display],
    display_id: u32,
    top_left: (f64, f64),
    size: (f64, f64),
) -> Option<Placement> {
    let d = displays
        .iter()
        .find(|d| d.id == display_id)
        .or_else(|| displays.first())?;
    Some(fit_on(d, top_left, size))
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
        assert_eq!(p.size, (500.0, 300.0), "cut to the usable width and height");
    }

    // ── refit: a panel resized in place (route switch, content change) ──

    const WORK: Rect = Rect {
        x: 0.0,
        y: 25.0,
        width: 1440.0,
        height: 805.0,
    };

    fn docked() -> Display {
        Display {
            id: 1,
            frame: rect(0.0, 0.0, 1440.0, 900.0),
            work: WORK,
        }
    }

    #[test]
    fn a_panel_that_fits_keeps_its_place_and_takes_its_new_height() {
        let p = refit(&[docked()], 1, (400.0, 200.0), (640.0, 300.0)).unwrap();
        assert_eq!((p.global, p.size), ((400.0, 200.0), (640.0, 300.0)));
        let p = refit(&[docked()], 1, (400.0, 200.0), (640.0, 140.0)).unwrap();
        assert_eq!(
            (p.global, p.size),
            ((400.0, 200.0), (640.0, 140.0)),
            "shrinks in place"
        );
    }

    #[test]
    fn growing_past_the_bottom_moves_the_panel_up() {
        // Top at 213 with 700 pt would end at 913, below the usable bottom (830).
        let p = refit(&[docked()], 1, (400.0, 213.0), (640.0, 700.0)).unwrap();
        assert_eq!(p.global, (400.0, 830.0 - 700.0));
        assert_eq!(p.size.1, 700.0);
    }

    #[test]
    fn taller_than_the_usable_area_is_cut_to_it_below_the_menu_bar() {
        let p = refit(&[docked()], 1, (400.0, 213.0), (640.0, 900.0)).unwrap();
        assert_eq!(p.global.1, 25.0);
        assert_eq!(p.size.1, 805.0);
    }

    #[test]
    fn refit_stays_on_the_panels_display() {
        let main = display(1, 0.0, 0.0, 1440.0, 900.0);
        let right = display(2, 1440.0, 0.0, 1920.0, 1080.0);
        let p = refit(&[main, right], 2, (2080.0, 216.0), (640.0, 400.0)).unwrap();
        assert_eq!(p.display_id, 2);
        assert_eq!(p.local, (640.0, 216.0), "display-local origin");
        // Its display is gone: the first one takes it, and the panel moves onto it
        // (its old x lay on the vanished display, right of the primary).
        let p = refit(&[main], 2, (2080.0, 216.0), (640.0, 400.0)).unwrap();
        assert_eq!(p.display_id, 1);
        assert_eq!(
            p.global,
            (1440.0 - 640.0, 216.0),
            "pulled back inside the work area"
        );
        let p = refit(&[main], 2, (-900.0, 216.0), (640.0, 400.0)).unwrap();
        assert_eq!(p.global.0, 0.0, "from the left too");
        // A display narrower than the panel cuts the width.
        let narrow = display(3, 0.0, 0.0, 500.0, 900.0);
        let p = refit(&[narrow], 3, (0.0, 100.0), (640.0, 200.0)).unwrap();
        assert_eq!((p.global.0, p.size.0), (0.0, 500.0));
        assert_eq!(refit(&[], 1, (0.0, 0.0), (640.0, 140.0)), None);
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
    fn a_top_left_rect_flips_to_an_appkit_origin_and_back() {
        // Primary 900 high: a 640x200 panel at top y=100 has its AppKit origin at
        // y = 900 - 300 = 600; flipping that rect back gives the same place.
        assert_eq!(
            appkit_origin((400.0, 100.0), (640.0, 200.0), 900.0),
            (400.0, 600.0)
        );
        let back = Rect::from_appkit(400.0, 600.0, 640.0, 200.0, 900.0);
        assert_eq!((back.x, back.y), (400.0, 100.0));
        // A display above the primary has negative top-left y.
        assert_eq!(
            appkit_origin((0.0, -1080.0), (640.0, 140.0), 900.0),
            (0.0, 1840.0)
        );
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
