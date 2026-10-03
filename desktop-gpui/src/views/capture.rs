//! The capture stub (Phase 0 R13). Phase 1 O2 ports this into the real capture view;
//! until then it renders only its title, through a normal GPUI text element rather
//! than a drawn glyph (the tray icon's link glyph, by contrast, is pixels with no text
//! for an accessibility tree to find) — so Phase 1's accessibility proofs have real
//! text here to anchor on. No business logic: this file is the only one its Phase 1
//! agent touches.
//!
//! The title goes through GPUI's `text!` macro, not a bare `&str`: in gpui-pre 0.3.7 a
//! `&str` child has no id and no accessibility role, so it never reaches the AccessKit
//! tree. Task 6's `a11y` verb showed this live — the tree held only the Window node
//! until the title became `text!` (a `Label` node whose value is the title). Phase 1
//! views need the same for any text a proof reads (spec D6).

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::Route;
use crate::theme::tokens::{font_size, radius, spacing};

/// The capture route's view-state, for Task 6's automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Capture(State {
        view: Route::Capture.as_str().into(),
        phase: "stub".into(),
    })
}

pub struct CaptureView;

impl CaptureView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        CaptureView
    }
}

impl Render for CaptureView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(spacing::S1)
            .p(spacing::S5)
            .rounded(radius::CARD)
            .child(
                div()
                    .text_size(font_size::TITLE)
                    .text_color(theme.foreground)
                    .child(text!(Route::Capture.title())),
            )
    }
}

/// This route's row in the router table (`views::TABLE`) builds its view from here.
pub fn build(cx: &mut App) -> AnyView {
    cx.new(CaptureView::new).into()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would re-import gpui-kit's `test` macro (pulled in
    // by this file's `use gpui_kit::*;`), which shadows Rust's `#[test]` and blows the
    // recursion limit expanding it (see shell/mod.rs's tests module for the same note).
    use super::state;

    #[test]
    fn state_names_its_own_route_and_stub_phase() {
        let json = state().to_json();
        assert_eq!(json["view"], "capture");
        assert_eq!(json["phase"], "stub");
    }
}
