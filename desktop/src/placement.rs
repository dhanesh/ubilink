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

/// Top-left corner, in points, for a panel of `panel` (width, height) points.
/// Uses the display under `cursor`; if the cursor is on none of them, the first
/// (the caller lists the primary display first). `None` when there are no
/// displays, meaning: leave the window where it is.
pub fn panel_origin(
    screens: &[Screen],
    cursor: (f64, f64),
    (width, height): (f64, f64),
) -> Option<(f64, f64)> {
    let s = screens
        .iter()
        .find(|s| s.contains(cursor))
        .or_else(|| screens.first())?;
    let x = s.x + ((s.width - width) / 2.0).max(0.0);
    // Near the top, but moved up rather than hanging off the bottom edge, and
    // never above the top edge.
    let y = (s.y + s.height * TOP_FRACTION)
        .min(s.y + s.height - height)
        .max(s.y);
    Some((x, y))
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

    #[test]
    fn single_screen_centres_horizontally_near_the_top() {
        let main = screen(0.0, 0.0, 1440.0, 900.0);
        let (x, y) = panel_origin(&[main], (100.0, 100.0), PANEL).unwrap();
        assert_eq!(x, (1440.0 - 640.0) / 2.0);
        assert_eq!(y, 900.0 * TOP_FRACTION);
    }

    #[test]
    fn opens_on_the_screen_under_the_cursor() {
        let main = screen(0.0, 0.0, 1440.0, 900.0);
        let right = screen(1440.0, 0.0, 1920.0, 1080.0);
        let (x, y) = panel_origin(&[main, right], (2000.0, 500.0), PANEL).unwrap();
        assert_eq!(x, 1440.0 + (1920.0 - 640.0) / 2.0);
        assert_eq!(y, 1080.0 * TOP_FRACTION);
    }

    #[test]
    fn handles_screens_left_of_and_above_the_primary() {
        // Negative global coordinates are normal on macOS for screens arranged
        // left of or above the primary display.
        let main = screen(0.0, 0.0, 1440.0, 900.0);
        let left = screen(-1920.0, -180.0, 1920.0, 1080.0);
        let (x, y) = panel_origin(&[main, left], (-500.0, 0.0), PANEL).unwrap();
        assert_eq!(x, -1920.0 + (1920.0 - 640.0) / 2.0);
        assert_eq!(y, -180.0 + 1080.0 * TOP_FRACTION);
    }

    #[test]
    fn cursor_off_every_screen_falls_back_to_the_first() {
        // The caller lists the primary display first.
        let main = screen(0.0, 0.0, 1440.0, 900.0);
        let right = screen(1440.0, 200.0, 1920.0, 1080.0);
        // In the gap above the right-hand screen, which starts lower.
        let (x, _) = panel_origin(&[main, right], (2000.0, 50.0), PANEL).unwrap();
        assert_eq!(x, (1440.0 - 640.0) / 2.0);
    }

    #[test]
    fn a_screen_edge_belongs_to_one_screen_only() {
        // x == 1440 is the right screen's first column, not the main screen's.
        let main = screen(0.0, 0.0, 1440.0, 900.0);
        let right = screen(1440.0, 0.0, 1920.0, 1080.0);
        let (x, _) = panel_origin(&[main, right], (1440.0, 10.0), PANEL).unwrap();
        assert_eq!(x, 1440.0 + (1920.0 - 640.0) / 2.0);
    }

    #[test]
    fn no_screens_means_leave_the_window_where_it_is() {
        assert_eq!(panel_origin(&[], (0.0, 0.0), PANEL), None);
    }

    #[test]
    fn never_spills_off_a_screen_smaller_than_the_panel() {
        let tiny = screen(100.0, 50.0, 500.0, 300.0);
        let (x, y) = panel_origin(&[tiny], (200.0, 100.0), (640.0, 900.0)).unwrap();
        assert_eq!(x, 100.0, "too wide: pin to the left edge");
        assert_eq!(y, 50.0, "too tall: pin to the top edge");
    }

    #[test]
    fn a_tall_panel_moves_up_rather_than_off_the_bottom() {
        let main = screen(0.0, 0.0, 1440.0, 900.0);
        let (_, y) = panel_origin(&[main], (10.0, 10.0), (640.0, 800.0)).unwrap();
        assert_eq!(y, 100.0, "bottom edge sits on the screen's bottom edge");
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
        let (x, _) = panel_origin(&[built_in, external], cursor, PANEL).unwrap();
        assert_eq!(
            x,
            1440.0 + (1920.0 - 640.0) / 2.0,
            "panel lands on the external"
        );
    }
}
