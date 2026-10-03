//! The onboarding stub (Phase 0 R13). Phase 1 O2 ports this into the real first-run
//! flow; until then it renders only its title, through a normal GPUI text element (see
//! `capture.rs`'s doc comment for why that matters for accessibility). No business
//! logic: this file is the only one its Phase 1 agent touches.
//!
//! Nothing in the shell opens this route yet (shell/mod.rs's doc comment on `Route`);
//! it is registered here ahead of that so Phase 1's first-run flow has a router row to
//! land in without re-touching this table.

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::Route;
use crate::theme::tokens::{font_size, radius, spacing};

/// The onboarding route's view-state, for Task 6's automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Onboarding(State {
        view: Route::Onboarding.as_str().into(),
        phase: "stub".into(),
    })
}

pub struct OnboardingView;

impl OnboardingView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        OnboardingView
    }
}

impl Render for OnboardingView {
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
                    .child(Route::Onboarding.title()),
            )
    }
}

/// This route's row in the router table (`views::TABLE`) builds its view from here.
pub fn build(cx: &mut App) -> AnyView {
    cx.new(OnboardingView::new).into()
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
        assert_eq!(json["view"], "onboarding");
        assert_eq!(json["phase"], "stub");
    }
}
