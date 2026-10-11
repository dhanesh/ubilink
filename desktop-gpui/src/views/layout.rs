//! Where a view's elements really are, for the probes (`state.view.layout`).
//!
//! The accessibility dump (gpui-pre 0.3.7 `debug_a11y_tree_json`) carries no bounds and
//! no text size, so "the last adapter toggle is inside the window", "the three action
//! buttons are one height" and "the status text is the label's size" cannot be read
//! from it. A view marks the elements those checks need with a zero-paint [`canvas`]
//! (the same technique as `theming::Samples`), which records at prepaint:
//!
//! - [`Layout::bounds`]: the parent's laid-out bounds in window points
//!   (`{x, y, w, h, bottom}`), plus the window's `viewport` (`{w, h}`) at that frame;
//! - [`Layout::font`]: the text size the parent's text actually resolves to
//!   (`{font_px}`), read from the text style GPUI applies to the parent's children;
//! - [`Layout::text_field`]: a text field's bounds, the box its text is clipped to and
//!   the line box its `InputState` laid the text out in this frame.
//!
//! Measured, never declared: a value is whatever the layout produced this frame. A
//! view clears its layout at the top of each render, so an element that is gone reads
//! as missing (and a probe step that needs it fails) instead of a stale place.
//!
//! Automation builds only: nothing but the automation `state` verb reads the layout, so
//! a release build gets the no-op [`Layout`] below. Its markers are `Empty` elements and
//! no store, lock or JSON is built at prepaint.

use std::collections::BTreeMap;

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use serde_json::Value;

#[cfg(feature = "automation")]
use serde_json::json;
#[cfg(feature = "automation")]
use std::sync::Mutex;

/// One view's measured elements, keyed by a name the probe uses as a path segment.
#[cfg(feature = "automation")]
pub struct Layout(Mutex<BTreeMap<&'static str, Value>>);

/// Release builds: measures nothing (see the module doc).
#[cfg(not(feature = "automation"))]
pub struct Layout;

#[cfg(not(feature = "automation"))]
impl Layout {
    pub const fn new() -> Self {
        Layout
    }

    /// Always empty: nothing is measured.
    #[allow(dead_code)]
    pub fn get(&self) -> BTreeMap<String, Value> {
        BTreeMap::new()
    }

    pub fn clear(&self) {}

    pub fn bounds(&'static self, _key: &'static str) -> impl IntoElement {
        Empty
    }

    pub fn font(&'static self, _key: &'static str) -> impl IntoElement {
        Empty
    }

    pub fn text_field(
        &'static self,
        _key: &'static str,
        _state: &Entity<InputState>,
        _inset_y: Pixels,
    ) -> impl IntoElement {
        Empty
    }
}

#[cfg(feature = "automation")]
impl Layout {
    pub const fn new() -> Self {
        Layout(Mutex::new(BTreeMap::new()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<&'static str, Value>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Everything measured in the latest frame, as the view-state's `layout` object.
    pub fn get(&self) -> BTreeMap<String, Value> {
        self.lock()
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// Forget every measurement: call at the top of each render.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn set(&self, key: &'static str, value: Value) {
        self.lock().insert(key, value);
    }

    /// A zero-paint element laid over its parent (which must be `relative()`) that
    /// records the parent's bounds as `key`, and the window's viewport as `viewport`.
    pub fn bounds(&'static self, key: &'static str) -> impl IntoElement {
        canvas(
            move |bounds, window, _| {
                self.set(key, rect_json(bounds));
                let v = window.viewport_size();
                self.set(
                    "viewport",
                    json!({"w": f32::from(v.width), "h": f32::from(v.height)}),
                );
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// A zero-size, zero-paint child that records the text size its parent's text
    /// resolves to (rems resolved against the window's rem size) as `key.font_px`.
    pub fn font(&'static self, key: &'static str) -> impl IntoElement {
        canvas(
            move |_, window, _| {
                let px = window.text_style().font_size.to_pixels(window.rem_size());
                self.set(key, json!({"font_px": round2(f32::from(px))}));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0()
    }

    /// A zero-paint element laid over a text field's wrapper (which must be
    /// `relative()` and hold only the field) that records, as `key`:
    ///
    /// - `field`: the field's laid-out bounds (the wrapper's);
    /// - `content`: those bounds less `inset_y` top and bottom (the field's border and
    ///   vertical padding, `theming::TEXT_FIELD_INSET_Y`): the box the single-line
    ///   text is clipped to;
    /// - `text`: the text line box the field's `InputState` laid out this frame (its
    ///   `input_bounds`, one line height tall), which the glyphs, descenders
    ///   included, are drawn in;
    /// - `centre_offset`: how far the line box's vertical centre is from the field's.
    ///
    /// Recorded at paint, after the field has painted and stored this frame's line box.
    pub fn text_field(
        &'static self,
        key: &'static str,
        state: &Entity<InputState>,
        inset_y: Pixels,
    ) -> impl IntoElement {
        let state = state.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, _, cx| {
                let text = state.read(cx).input_bounds();
                let content = Bounds::from_corners(
                    point(bounds.left(), bounds.top() + inset_y),
                    point(bounds.right(), bounds.bottom() - inset_y),
                );
                let centre = |b: Bounds<Pixels>| f32::from(b.top() + b.size.height / 2.);
                self.set(
                    key,
                    json!({
                        "field": rect_json(bounds),
                        "content": rect_json(content),
                        "text": rect_json(text),
                        "centre_offset": round2((centre(text) - centre(bounds)).abs()),
                    }),
                );
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// Points rounded to 1/100: layout floats compare equal when the layout is equal.
#[cfg(feature = "automation")]
fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// `{x, y, w, h, bottom}` in window points, rounded to 1/100.
#[cfg(feature = "automation")]
pub fn rect_json(b: Bounds<Pixels>) -> Value {
    let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    json!({
        "x": round2(x),
        "y": round2(y),
        "w": round2(w),
        "h": round2(h),
        "bottom": round2(y + h),
    })
}

#[cfg(all(test, feature = "automation"))]
mod tests {
    use super::{rect_json, Layout};
    use gpui_kit::{point, px, size, Bounds};
    use serde_json::json;

    #[test]
    fn a_rect_reports_its_bottom_edge_rounded() {
        let b = Bounds {
            origin: point(px(20.), px(100.333)),
            size: size(px(140.), px(16.)),
        };
        assert_eq!(
            rect_json(b),
            json!({"x": 20.0, "y": 100.33, "w": 140.0, "h": 16.0, "bottom": 116.33})
        );
    }

    #[test]
    fn clear_forgets_every_measurement() {
        static L: Layout = Layout::new();
        L.set("a", json!(1));
        assert_eq!(L.get().len(), 1);
        L.clear();
        assert!(L.get().is_empty());
    }
}
