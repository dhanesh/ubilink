//! The browser stub (Phase 0 R13). Phase 1 O2 ports this into the real link-browser
//! view; until then it renders only its title, through GPUI's accessible `text!`
//! element (see `capture.rs`'s doc comment for why that matters for accessibility). No business
//! logic: this file is the only one its Phase 1 agent touches.

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::Route;
use crate::theme::tokens::{font_size, radius, spacing};

/// The browser route's view-state, for Task 6's automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Browser(State {
        view: Route::Browser.as_str().into(),
        phase: "stub".into(),
    })
}

pub struct BrowserView;

impl BrowserView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        BrowserView
    }
}

impl Render for BrowserView {
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
                    .child(text!(Route::Browser.title())),
            )
    }
}

/// This route's row in the router table (`views::TABLE`) builds its view from here.
pub fn build(cx: &mut App) -> AnyView {
    cx.new(BrowserView::new).into()
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
        assert_eq!(json["view"], "browser");
        assert_eq!(json["phase"], "stub");
    }
}
