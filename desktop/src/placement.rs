//! Where the panel opens: on the display under the mouse cursor, centred
//! horizontally with its top edge a fifth of the way down (Spotlight-style), and
//! never spilling off that display. Pure, so the multi-display cases — which need
//! hardware to try by hand — are covered by tests.
//!
//! Everything here is in global POINTS with a top-left origin (macOS's
//! CGDisplayBounds space), the only space where every display agrees. tao's macOS
//! "physical" values are not one space: a monitor's position and size are points
//! times THAT monitor's scale, and the cursor is points times the PRIMARY
//! monitor's scale. See `Screen::from_physical` and `cursor_points`.

/// How far down the display the panel's top edge sits, as a fraction of height.
pub const TOP_FRACTION: f64 = 0.2;

/// One display's frame in global points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Screen {
    /// From a Tauri/tao monitor's reported position and size, which on macOS are
    /// that monitor's point frame multiplied by its own scale factor.
    pub fn from_physical(x: i32, y: i32, width: u32, height: u32, scale: f64) -> Screen {
        Screen {
            x: f64::from(x) / scale,
            y: f64::from(y) / scale,
            width: f64::from(width) / scale,
            height: f64::from(height) / scale,
        }
    }

    /// Half-open, so a shared edge belongs to exactly one display.
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// The cursor in global points, from tao's macOS cursor position, which is the
/// point position multiplied by the PRIMARY monitor's scale factor.
pub fn cursor_points(x: f64, y: f64, primary_scale: f64) -> (f64, f64) {
    (x / primary_scale, y / primary_scale)
}

/// Gap kept between the panel and the edges of a display's usable area when the
/// panel has to be moved or capped to fit.
pub const EDGE: f64 = 8.0;

/// A display: its whole frame, and the part of it windows may use (the frame
/// minus the menu bar and Dock). Both in global points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub frame: Screen,
    pub work: Screen,
}

/// Top-left corner, in points, for a panel of `panel` (width, height) points.
/// The display is chosen by its FRAME, so a cursor up in the menu bar still
/// counts as on that display; the panel is placed within its usable area. If the
/// cursor is on no display, the first is used (the caller lists the primary
/// display first). `None` when there are no displays: leave the window alone.
pub fn panel_origin(
    displays: &[Display],
    cursor: (f64, f64),
    (width, height): (f64, f64),
) -> Option<(f64, f64)> {
    let s = displays
        .iter()
        .find(|d| d.frame.contains(cursor))
        .or_else(|| displays.first())?
        .work;
    let x = s.x + ((s.width - width) / 2.0).max(0.0);
    // Near the top, but moved up rather than hanging off the bottom edge, and
    // never above the top edge.
    let y = (s.y + s.height * TOP_FRACTION)
        .min(s.y + s.height - height)
        .max(s.y);
    Some((x, y))
}

/// Keep a panel whose content wants `wanted` points of height on-screen, given
/// its current `top` and the usable area `work` of the display it is on. Returns
/// the (top, height) to apply: the height capped to what fits (the panel scrolls
/// inside), and the top moved up if the panel would run past the bottom, or down
/// if it sits above the usable area. A panel that already fits is left alone.
pub fn fit_vertically(work: Screen, top: f64, wanted: f64) -> (f64, f64) {
    let height = wanted.min((work.height - 2.0 * EDGE).max(0.0));
    let lowest_top = work.y + work.height - EDGE - height;
    let top = top.min(lowest_top).max(work.y + EDGE);
    (top, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: (f64, f64) = (640.0, 140.0);

    fn screen(x: f64, y: f64, width: f64, height: f64) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }
    /// A display whose usable area is its whole frame (no menu bar or Dock).
    fn display(x: f64, y: f64, width: f64, height: f64) -> Display {
        let s = screen(x, y, width, height);
        Display { frame: s, work: s }
    }

    #[test]
    fn single_screen_centres_horizontally_near_the_top() {
        let main = display(0.0, 0.0, 1440.0, 900.0);
        let (x, y) = panel_origin(&[main], (100.0, 100.0), PANEL).unwrap();
        assert_eq!(x, (1440.0 - 640.0) / 2.0);
        assert_eq!(y, 900.0 * TOP_FRACTION);
    }

    #[test]
    fn opens_on_the_screen_under_the_cursor() {
        let main = display(0.0, 0.0, 1440.0, 900.0);
        let right = display(1440.0, 0.0, 1920.0, 1080.0);
        let (x, y) = panel_origin(&[main, right], (2000.0, 500.0), PANEL).unwrap();
        assert_eq!(x, 1440.0 + (1920.0 - 640.0) / 2.0);
        assert_eq!(y, 1080.0 * TOP_FRACTION);
    }

    #[test]
    fn handles_screens_left_of_and_above_the_primary() {
        // Negative global coordinates are normal on macOS for screens arranged
        // left of or above the primary display.
        let main = display(0.0, 0.0, 1440.0, 900.0);
        let left = display(-1920.0, -180.0, 1920.0, 1080.0);
        let (x, y) = panel_origin(&[main, left], (-500.0, 0.0), PANEL).unwrap();
        assert_eq!(x, -1920.0 + (1920.0 - 640.0) / 2.0);
        assert_eq!(y, -180.0 + 1080.0 * TOP_FRACTION);
    }

    #[test]
    fn cursor_off_every_screen_falls_back_to_the_first() {
        // The caller lists the primary display first.
        let main = display(0.0, 0.0, 1440.0, 900.0);
        let right = display(1440.0, 200.0, 1920.0, 1080.0);
        // In the gap above the right-hand screen, which starts lower.
        let (x, _) = panel_origin(&[main, right], (2000.0, 50.0), PANEL).unwrap();
        assert_eq!(x, (1440.0 - 640.0) / 2.0);
    }

    #[test]
    fn a_screen_edge_belongs_to_one_screen_only() {
        // x == 1440 is the right screen's first column, not the main screen's.
        let main = display(0.0, 0.0, 1440.0, 900.0);
        let right = display(1440.0, 0.0, 1920.0, 1080.0);
        let (x, _) = panel_origin(&[main, right], (1440.0, 10.0), PANEL).unwrap();
        assert_eq!(x, 1440.0 + (1920.0 - 640.0) / 2.0);
    }

    #[test]
    fn no_screens_means_leave_the_window_where_it_is() {
        assert_eq!(panel_origin(&[], (0.0, 0.0), PANEL), None);
    }

    #[test]
    fn never_spills_off_a_screen_smaller_than_the_panel() {
        let tiny = display(100.0, 50.0, 500.0, 300.0);
        let (x, y) = panel_origin(&[tiny], (200.0, 100.0), (640.0, 900.0)).unwrap();
        assert_eq!(x, 100.0, "too wide: pin to the left edge");
        assert_eq!(y, 50.0, "too tall: pin to the top edge");
    }

    #[test]
    fn a_tall_panel_moves_up_rather_than_off_the_bottom() {
        let main = display(0.0, 0.0, 1440.0, 900.0);
        let (_, y) = panel_origin(&[main], (10.0, 10.0), (640.0, 800.0)).unwrap();
        assert_eq!(y, 100.0, "bottom edge sits on the screen's bottom edge");
    }

    #[test]
    fn places_within_the_usable_area_but_picks_the_display_by_its_frame() {
        // The menu bar (top 25 pt) and the Dock (bottom 70 pt) are part of the
        // display's frame but not its usable area. A cursor up in the menu bar
        // is still on this display; the panel is centred in, and kept inside,
        // the usable area.
        let d = Display {
            frame: screen(0.0, 0.0, 1440.0, 900.0),
            work: screen(0.0, 25.0, 1440.0, 805.0),
        };
        let (x, y) = panel_origin(&[d], (700.0, 5.0), PANEL).unwrap();
        assert_eq!(x, (1440.0 - 640.0) / 2.0);
        assert_eq!(y, 25.0 + 805.0 * TOP_FRACTION);
        let (_, y) = panel_origin(&[d], (700.0, 5.0), (640.0, 800.0)).unwrap();
        assert_eq!(
            y,
            25.0 + 805.0 - 800.0,
            "clear of the Dock, not the frame bottom"
        );
    }

    #[test]
    fn mixed_retina_layout_resolves_to_the_right_screen() {
        // The case that breaks a naive implementation, and the one this machine
        // can't exercise live. tao reports a monitor's position and size as
        // points x THAT monitor's scale, and the cursor as points x the PRIMARY
        // monitor's scale. A 2x built-in (1440x900 pt) with a 1x external to its
        // right therefore reports the external at physical x=1440 — inside the
        // built-in's physical span (0..2880). Compared in physical pixels, a
        // cursor on the external would match the built-in.
        let built_in = Screen::from_physical(0, 0, 2880, 1800, 2.0);
        let external = Screen::from_physical(1440, 0, 1920, 1080, 1.0);
        assert_eq!(built_in, screen(0.0, 0.0, 1440.0, 900.0));
        assert_eq!(external, screen(1440.0, 0.0, 1920.0, 1080.0));

        // Cursor 260 pt into the external: reported as (1700, 200) pt x 2.
        let cursor = cursor_points(3400.0, 400.0, 2.0);
        assert_eq!(cursor, (1700.0, 200.0));
        let displays = [
            Display {
                frame: built_in,
                work: built_in,
            },
            Display {
                frame: external,
                work: external,
            },
        ];
        let (x, _) = panel_origin(&displays, cursor, PANEL).unwrap();
        assert_eq!(
            x,
            1440.0 + (1920.0 - 640.0) / 2.0,
            "panel lands on the external"
        );
    }

    // ── fit_vertically: keeping the panel on-screen as its content grows ──

    const WORK: Screen = Screen {
        x: 0.0,
        y: 25.0,
        width: 1440.0,
        height: 805.0,
    };

    #[test]
    fn a_panel_that_fits_keeps_its_place_and_height() {
        assert_eq!(fit_vertically(WORK, 200.0, 300.0), (200.0, 300.0));
    }

    #[test]
    fn growing_past_the_bottom_moves_the_panel_up() {
        // Settings grew to 700 pt with its top at 213: it would end at 913, below
        // the usable bottom (830). It moves up so its bottom sits EDGE above it.
        let (top, height) = fit_vertically(WORK, 213.0, 700.0);
        assert_eq!(height, 700.0);
        assert_eq!(top, 25.0 + 805.0 - EDGE - 700.0);
    }

    #[test]
    fn content_taller_than_the_display_is_capped_and_scrolls() {
        let (top, height) = fit_vertically(WORK, 213.0, 2000.0);
        assert_eq!(height, 805.0 - 2.0 * EDGE, "capped to the usable height");
        assert_eq!(top, 25.0 + EDGE, "pinned just below the menu bar");
    }

    #[test]
    fn a_panel_dragged_above_the_usable_area_comes_back_down() {
        assert_eq!(fit_vertically(WORK, -50.0, 300.0), (25.0 + EDGE, 300.0));
    }
}
