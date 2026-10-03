//! Task 7's accessibility spike (Phase 0 R7, TN5; Phase 1 U4 depends on the answer).
//! Automation builds only (this whole file is reachable only through the
//! `#[cfg(feature = "automation")] pub mod a11y_spike;` line in `views/mod.rs`).
//!
//! Phase 1's unattended proofs read the app's AccessKit tree through the automation
//! `a11y` verb (`automation/mod.rs`) to see what is on screen. Task 6 proved a stub
//! view's `text!` title reaches that tree as a `Label` node, and that a plain `&str`
//! child does not (GPUI pushes an accessibility node for an element only when it has
//! both an id and a non-`GenericContainer` role — see gpui-pre 0.3.7's
//! `ElementDrawPhase::prepaint` and `_accessibility.rs`'s note on
//! `note_focus_without_node`). Nobody had checked gpui-component 0.7.0's `List`,
//! `Input`, `Switch` and `Dialog` the same way before this task. This view renders
//! one of each with distinctive text; `probes/a11y-spike.json` asserts each one
//! reaches the tree. The findings — List, Input and Switch reach the tree with
//! their plain documented API; Dialog's title/content text does too, but its own
//! container never gets a role (a confirmed gpui-pre 0.3.7 gap, not fixable from a
//! caller, pinned in the scenario as an asserted-absent check rather than silently
//! omitted) — are in `probes/A11Y-NOTES.md`.
//!
//! What each widget turns out to do, confirmed by reading gpui-component's and
//! gpui-base's source (not guessed from behaviour):
//! - `List`/`ListState`: each row is wrapped by the widget itself in a `div` that
//!   carries `.role(Role::ListItem)` and `.aria_selected(..)` (`list/list.rs`,
//!   `render_list_item`). The row's own visible text still needs `text!` (same rule
//!   as Task 6) because a bare `&str` child never gets a role of its own; `text!`'s
//!   `Label` node nests under the row's `ListItem` node once rendered inside it.
//! - `Input`/`InputState`: `Input`'s `render` always calls `.role(accessibility_role)`
//!   (`input/input.rs`), and exposes the live text as the node's value whenever the
//!   a11y tree is active (`accessibility_value`, gated on `window.is_a11y_active()`).
//!   No extra call needed.
//! - `Switch`: wraps `gpui_base::Switch`, which sets `.role(Role::Switch)` and
//!   `.aria_toggled(..)` unconditionally, and falls back to the visible `.label(..)`
//!   text as the accessible name when `.accessibility_label(..)` isn't set
//!   (`switch.rs`). No extra call needed.
//! - `Dialog`: `gpui_kit::component::dialog::Dialog`, opened through
//!   `Window::open_dialog` (`gpui_component::WindowExt`), is NOT rendered inline —
//!   it is mounted by `WindowState`, a `RootPlugin` gpui-component registers via
//!   `gpui_component::init` (called from `main.rs`'s `gpui_kit::init(cx)`) onto the
//!   window's `gpui_base::Root`. `gpui_kit::open_window` (what `shell/mod.rs` uses to
//!   open the panel) always wraps its content in that `Root`, so the plugin — and
//!   `open_dialog` — are present. Its title goes through `text!`, same as a list row,
//!   and reaches the tree. The dialog's own surface does not: `gpui_base::Dialog`
//!   (what `Dialog` wraps) chains `.focus_trap(..)` on its role-bearing `div`, and
//!   `FocusTrapContainer::prepaint` forwards straight to that div's raw
//!   `Element::prepaint` instead of through gpui-pre's `Drawable` wrapper — the only
//!   place an a11y node gets pushed. The div's children (title, buttons) are
//!   unaffected, since `Div::prepaint` wraps each of them in their own `Drawable` as
//!   usual; only the dialog surface itself, and anything else using
//!   `.focus_trap(..)`, loses its node. See `probes/A11Y-NOTES.md`.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, IndexPath, WindowExt};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::Route;
use crate::theme::tokens::{font_size, radius, spacing};

/// The spike route's view-state, for Task 6's automation `state` verb. Same shape as
/// every other stub view's — Phase 1 does not port this one, so its state carries no
/// extra fields of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::A11ySpike(State {
        view: Route::A11ySpike.as_str().into(),
        phase: "stub".into(),
    })
}

/// Three fixed rows, "Spike row alpha/beta/gamma". `render_item` is the only required
/// piece of behaviour beyond the item count; everything else (role, selection state,
/// position-in-set) is `List`'s own job (see this file's module doc).
struct SpikeListDelegate;

impl ListDelegate for SpikeListDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        3
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let label = match ix.row {
            0 => "Spike row alpha",
            1 => "Spike row beta",
            _ => "Spike row gamma",
        };
        // `text!`, not a bare `&str` child: see this file's module doc and
        // `capture.rs`'s doc comment for why a plain string never reaches AccessKit.
        Some(ListItem::new(format!("spike-row-{}", ix.row)).child(text!(label)))
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        // The spike never reacts to a selection change; it only needs the row
        // `List` already marks as selected (set once below) to carry the flag.
        cx.notify();
    }
}

pub struct A11ySpikeView {
    list: Option<Entity<ListState<SpikeListDelegate>>>,
    input: Option<Entity<InputState>>,
    /// "Spike switch", on — a plain field, not an entity: `Switch` is a stateless
    /// `RenderOnce` element (gpui-component owns no state for it; the caller does).
    switch_on: bool,
    /// Guards `window.open_dialog` so the spike opens its dialog once, on the
    /// view's first render (the closest this view gets to "on creation": `build`
    /// below has no `Window` to open a dialog with, only `Render::render` does),
    /// rather than stacking a new one on every frame.
    dialog_opened: bool,
}

impl A11ySpikeView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        A11ySpikeView {
            list: None,
            input: None,
            switch_on: true,
            dialog_opened: false,
        }
    }
}

impl Render for A11ySpikeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.dialog_opened {
            self.dialog_opened = true;
            // The real widget (module doc): `WindowState` mounts it above this
            // view's own content, so it is never a child of the div tree below.
            window.open_dialog(cx, |dialog, _, _| dialog.title(text!("Spike dialog")));
        }

        let list = self
            .list
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut state = ListState::new(SpikeListDelegate, window, cx);
                    // Alpha (row 0) carries the selected flag the probe checks for.
                    state.set_selected_index(Some(IndexPath::new(0)), window, cx);
                    state
                })
            })
            .clone();
        let input = self
            .input
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Spike input value")
                        .default_value("Spike input value")
                })
            })
            .clone();

        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(spacing::S3)
            .p(spacing::S5)
            .rounded(radius::CARD)
            .child(
                div()
                    .text_size(font_size::TITLE)
                    .text_color(theme.foreground)
                    .child(text!(Route::A11ySpike.title())),
            )
            .child(div().h(px(96.)).child(List::new(&list)))
            .child(Input::new(&input))
            .child(
                Switch::new("spike-switch")
                    .label("Spike switch")
                    .checked(self.switch_on),
            )
    }
}

/// This route's row in the router table (`views::TABLE`) builds its view from here.
/// No `Window` is available here (`RouteEntry::build`'s signature, shared by every
/// route, has none) — the list, input and dialog, which all need one, are built
/// lazily on first `render` instead. See this file's module doc.
pub fn build(cx: &mut App) -> AnyView {
    cx.new(A11ySpikeView::new).into()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would re-import gpui-kit's `test` macro (pulled in by this
    // file's `use gpui_kit::*;`), which shadows Rust's `#[test]` and blows the
    // recursion limit expanding it (see shell/mod.rs's tests module for the same
    // note).
    use super::state;

    #[test]
    fn state_names_its_own_route_and_stub_phase() {
        let json = state().to_json();
        assert_eq!(json["view"], "a11y_spike");
        assert_eq!(json["phase"], "stub");
    }
}
