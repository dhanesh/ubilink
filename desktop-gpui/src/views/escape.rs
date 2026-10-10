//! What Escape does, per route (B1: the Tauri panel's keydown handler,
//! `ee80e70:dist/panel.js` 555-592), as one pure decision every view asks:
//!
//! - settings: close the reset confirmation if it is open, else dismiss;
//! - browser: cancel a pending ⌘L mark, else clear a non-empty filter, else dismiss;
//! - capture and onboarding (Tauri's generic `if (e.key === 'Escape') dismiss_panel`):
//!   dismiss.
//!
//! Dismiss hides the panel through the shell's own `Command::Hide`, the path a hotkey
//! toggle and the capture view's self-dismiss after a copy take, so the window really
//! leaves the screen ([`dismiss_panel`]).

use gpui_kit::{Context, Keystroke};

use super::Route;

/// What the view on screen knows that changes Escape's meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EscapeState {
    /// Settings: the reset confirmation is showing.
    pub confirming_reset: bool,
    /// Browser: a link is marked with ⌘L, waiting for its partner.
    pub marked: bool,
    /// Browser: the filter has text.
    pub filtering: bool,
}

/// What one Escape does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeAction {
    /// Settings: hide the reset confirmation; nothing is deleted.
    CloseConfirmation,
    /// Browser: drop the pending mark.
    Unmark,
    /// Browser: empty the filter, so every row shows again.
    ClearFilter,
    /// Hide the panel.
    Dismiss,
}

/// Plain Escape: the Tauri handler matched `e.key === 'Escape'`; a modified Escape is
/// some other shortcut.
pub fn is_escape(keystroke: &Keystroke) -> bool {
    keystroke.key == "escape" && !keystroke.modifiers.modified()
}

/// The Tauri panel's branches, in its order.
pub fn escape_action(route: Route, state: EscapeState) -> EscapeAction {
    match route {
        Route::Settings if state.confirming_reset => EscapeAction::CloseConfirmation,
        Route::Browser if state.marked => EscapeAction::Unmark,
        Route::Browser if state.filtering => EscapeAction::ClearFilter,
        _ => EscapeAction::Dismiss,
    }
}

/// Hide the panel as a dismiss does, once the current key's dispatch is over: the
/// shell removes the very window that is dispatching it.
pub fn dismiss_panel<V: 'static>(cx: &mut Context<V>) {
    cx.spawn(async move |_, cx| {
        cx.update(|cx| crate::shell::execute(crate::shell::Command::Hide, cx));
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{escape_action, is_escape, EscapeAction, EscapeState};
    use crate::views::Route;

    #[test]
    fn escape_follows_the_tauri_panels_branches() {
        let none = EscapeState::default();
        let confirming = EscapeState {
            confirming_reset: true,
            ..none
        };
        let marked = EscapeState {
            marked: true,
            ..none
        };
        let filtering = EscapeState {
            filtering: true,
            ..none
        };
        let both = EscapeState {
            marked: true,
            filtering: true,
            ..none
        };

        // Capture and onboarding: always dismiss.
        for r in [Route::Capture, Route::Onboarding] {
            for s in [none, confirming, marked, filtering, both] {
                assert_eq!(escape_action(r, s), EscapeAction::Dismiss, "{r:?} {s:?}");
            }
        }
        // Settings: the confirmation first, else dismiss.
        assert_eq!(
            escape_action(Route::Settings, confirming),
            EscapeAction::CloseConfirmation
        );
        assert_eq!(escape_action(Route::Settings, none), EscapeAction::Dismiss);
        assert_eq!(
            escape_action(Route::Settings, marked),
            EscapeAction::Dismiss
        );
        // Browser: the mark first, then the filter, then dismiss.
        assert_eq!(escape_action(Route::Browser, both), EscapeAction::Unmark);
        assert_eq!(escape_action(Route::Browser, marked), EscapeAction::Unmark);
        assert_eq!(
            escape_action(Route::Browser, filtering),
            EscapeAction::ClearFilter
        );
        assert_eq!(escape_action(Route::Browser, none), EscapeAction::Dismiss);
        assert_eq!(
            escape_action(Route::Browser, confirming),
            EscapeAction::Dismiss
        );
    }

    #[test]
    fn only_a_plain_escape_counts() {
        let key = |k: &str| gpui_kit::Keystroke::parse(k).unwrap();
        assert!(is_escape(&key("escape")));
        for k in ["cmd-escape", "shift-escape", "alt-escape", "enter", "left"] {
            assert!(!is_escape(&key(k)), "{k}");
        }
    }
}
