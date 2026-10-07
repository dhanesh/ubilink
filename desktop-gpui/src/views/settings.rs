//! The settings view (Phase 1 R10): App adapters — one `Switch` per adapter, toggled
//! from the keyboard and persisted to `plugins-state.json` (ubilink-app's
//! `{"disabled": [lowercased app names]}`) in the instance's data dir, so the choice
//! survives a restart. The Tauri panel's Settings › Plugins (`dist/panel.js`,
//! `renderPlugins`) is the behaviour being ported (B1).
//!
//! What it renders (probes/README.md, "Settings (R10-R13)"):
//! - one gpui-component `Switch` per adapter, its accessible name exactly the app name
//!   (`.label(app)`, A11Y-NOTES.md), on unless the user disabled it. Built-in adapters
//!   (Finder, Safari) are shown locked on: a disabled `Switch` takes no focus, so Tab
//!   skips it, and ubilink-app refuses to disable a built-in anyway.
//! - a hint line: what the switches do, or what failed and what to do (U3).
//!
//! Where the state lives: the one shared [`crate::app_state::get`] — never a copy —
//! so a toggle here reaches capture (which skips disabled adapters) at once. The rows
//! are re-read from it on every render through `ubilink_app::list_plugins`, and a
//! toggle goes through `ubilink_app::set_plugin_enabled`, which updates the shared set
//! and writes `plugins-state.json` through `paths::data_dir()` (the state's data dir).
//!
//! Keyboard: the view focuses itself when shown, so Tab (gpui-base `Root`'s
//! `focus_next`) reaches the first switch. The automation `key` verb only sends
//! key-down events, while GPUI's own Space/Enter-activates-a-focused-switch path fires
//! on key *up*; so each switch sits in a wrapper whose `capture_key_down` toggles it on
//! Space/Enter while the switch inside is focused, and calls `prevent_default` so the
//! switch's own key-up activation does not toggle it straight back for a real keyboard
//! user (the same pattern as `onboarding.rs`'s step buttons).

use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use ubilink_app::PluginView;

use super::Route;
use crate::theme::tokens::{font_size, radius, spacing};

/// The settings route's view-state, for Task 6's automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Settings(State {
        view: Route::Settings.as_str().into(),
        phase: "ready".into(),
    })
}

const ADAPTERS_HINT: &str =
    "Turn off an app to stop capturing from it. Built-in apps are always on.";

/// The line shown after a toggle: what changed, or what failed and what to do (U3).
pub fn toggle_message(app: &str, enabled: bool, result: &Result<(), String>) -> String {
    match result {
        Ok(()) if enabled => format!("{app} is on: ubilink captures links from it again."),
        Ok(()) => format!("{app} is off: ubilink no longer captures links from it."),
        Err(e) => format!(
            "Couldn't turn {app} {}: {e} Check that ubilink can write its data folder, then try again.",
            if enabled { "on" } else { "off" }
        ),
    }
}

/// The message to show when the adapter list itself cannot be read (U3).
pub fn list_error_message(e: &str) -> String {
    format!("Couldn't read the app adapters: {e}. Reopen Settings to try again.")
}

/// Turn `app` on or off in `state` and persist it to `plugins-state.json`; the
/// result line to show, and whether it failed.
pub fn toggle(state: &ubilink_app::AppState, app: &str, enabled: bool) -> (String, bool) {
    let result = ubilink_app::set_plugin_enabled(state, app, enabled);
    (toggle_message(app, enabled, &result), result.is_err())
}

/// Space or Enter with no modifiers: the keys that activate a focused switch.
fn is_activation_key(keystroke: &Keystroke) -> bool {
    (keystroke.key == "space" || keystroke.key == "enter") && !keystroke.modifiers.modified()
}

/// A stable element id per adapter (keys the switch's focus state across renders).
fn switch_id(app: &str) -> SharedString {
    SharedString::from(format!("adapter-{}", app.to_ascii_lowercase()))
}

pub struct SettingsView {
    /// The last toggle's result, or an error; `None` shows the standing hint.
    message: Option<(String, bool)>,
    /// Held so the view handles keys from its first frame: GPUI dispatches keys from
    /// the focused element up, and with nothing focused Tab never arrives.
    focus: FocusHandle,
    focused: bool,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        SettingsView {
            message: None,
            focus: cx.focus_handle(),
            focused: false,
        }
    }

    fn set_enabled(&mut self, app: &str, enabled: bool, cx: &mut Context<Self>) {
        self.message = Some(toggle(crate::app_state::get(), app, enabled));
        cx.notify();
    }

    fn adapter_switch(&self, plugin: &PluginView, cx: &mut Context<Self>) -> AnyElement {
        let app: SharedString = plugin.app.clone().into();
        let enabled = plugin.enabled;
        let locked = plugin.builtin;
        let key_app = app.clone();
        let click_app = app.clone();
        div()
            .w(px(140.))
            .flex_shrink_0()
            // Space/Enter reach this wrapper only while the switch inside it is
            // focused (keys go from the focused element up); see the module doc.
            .when(!locked, |this| {
                this.capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if is_activation_key(&event.keystroke) {
                        this.set_enabled(&key_app, !enabled, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }))
            })
            .child(
                Switch::new(switch_id(&app))
                    .small()
                    .label(app.clone())
                    .checked(enabled)
                    .disabled(locked)
                    .when(locked, |s| s.tooltip("Built-in: always on"))
                    .on_click(cx.listener(move |this, next: &bool, _, cx| {
                        this.set_enabled(&click_app, *next, cx)
                    })),
            )
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        let (switches, list_error) = match ubilink_app::list_plugins(crate::app_state::get()) {
            Ok(plugins) => (
                plugins
                    .iter()
                    .map(|p| self.adapter_switch(p, cx))
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(e) => (Vec::new(), Some(list_error_message(&e))),
        };
        let (hint, failed) = match (&list_error, &self.message) {
            (Some(e), _) => (e.clone(), true),
            (None, Some((m, failed))) => (m.clone(), *failed),
            (None, None) => (ADAPTERS_HINT.to_string(), false),
        };
        let theme = cx.theme();
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .gap(spacing::S1)
            .px(spacing::S5)
            .py(spacing::S2)
            .rounded(radius::CARD)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(spacing::S3)
                    .child(
                        div()
                            .text_size(font_size::TITLE)
                            .text_color(theme.foreground)
                            .child(text!(Route::Settings.title())),
                    )
                    .child(
                        div()
                            .text_size(font_size::META)
                            .text_color(theme.muted_foreground)
                            .child(text!(id = "settings-adapters-heading", "App adapters")),
                    ),
            )
            .child(
                div()
                    .id("settings-adapters")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .content_start()
                    .gap_x(spacing::S2)
                    .gap_y(spacing::S2)
                    .children(switches),
            )
            .child(
                div()
                    .truncate()
                    .text_size(font_size::META)
                    .text_color(if failed {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    })
                    .child(text!(id = "settings-hint", hint)),
            )
    }
}

/// This route's row in the router table (`views::TABLE`) builds its view from here.
pub fn build(cx: &mut App) -> AnyView {
    cx.new(SettingsView::new).into()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would re-import gpui-kit's `test` macro (pulled in
    // by this file's `use gpui_kit::*;`), which shadows Rust's `#[test]` and blows the
    // recursion limit expanding it (see shell/mod.rs's tests module for the same note).
    use super::{list_error_message, state, toggle, toggle_message};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use ubilink_app::{list_plugins, load_disabled, AppState, HotkeyReport};
    use ubilink_core::{AdapterRegistry, LinkGraph, PanelBackend, UbilinkService};

    fn temp_state(tag: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "ubilink-next-settings-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        AppState {
            backend: Mutex::new(PanelBackend::new(UbilinkService::new(
                LinkGraph::open_in_memory().unwrap(),
            ))),
            adapters: AdapterRegistry::defaults(),
            disabled: Mutex::new(load_disabled(&dir)),
            data_dir: dir,
            hotkeys: Mutex::new(HotkeyReport::default()),
        }
    }

    #[test]
    fn state_names_its_own_route() {
        let json = state().to_json();
        assert_eq!(json["view"], "settings");
        assert_eq!(json["phase"], "ready");
    }

    /// R10: every non-built-in adapter toggles off and back on, each change lands in
    /// `plugins-state.json`, and a fresh state read from that file (a restart) sees it.
    #[test]
    fn settings_adapters_toggle_and_persist_across_a_restart() {
        let state = temp_state("toggle");
        let toggleable: Vec<String> = list_plugins(&state)
            .unwrap()
            .into_iter()
            .filter(|p| !p.builtin)
            .map(|p| p.app)
            .collect();
        assert!(!toggleable.is_empty());
        assert!(!state.data_dir.join("plugins-state.json").exists());

        for app in &toggleable {
            let (msg, failed) = toggle(&state, app, false);
            assert!(!failed && msg.contains("is off"), "{msg}");
        }
        let after_restart = load_disabled(&state.data_dir);
        let expected: HashSet<String> = toggleable.iter().map(|a| a.to_ascii_lowercase()).collect();
        assert_eq!(after_restart, expected);

        for app in &toggleable {
            toggle(&state, app, true);
        }
        assert!(load_disabled(&state.data_dir).is_empty());
        assert!(list_plugins(&state).unwrap().iter().all(|p| p.enabled));
        std::fs::remove_dir_all(&state.data_dir).ok();
    }

    /// Built-ins stay locked on, and a refusal says what failed and what to do (U3).
    #[test]
    fn settings_builtins_stay_on_and_failures_say_what_to_do() {
        let state = temp_state("builtin");
        let builtin = list_plugins(&state)
            .unwrap()
            .into_iter()
            .find(|p| p.builtin)
            .expect("the defaults ship built-in adapters");
        assert!(builtin.enabled);
        let (msg, failed) = toggle(&state, &builtin.app, false);
        assert!(failed);
        assert!(
            msg.starts_with(&format!("Couldn't turn {} off", builtin.app)),
            "{msg}"
        );
        assert!(msg.contains("try again"), "{msg}");
        assert!(list_plugins(&state)
            .unwrap()
            .iter()
            .any(|p| p.app == builtin.app && p.enabled));

        assert!(toggle_message("Arc", true, &Ok(())).contains("Arc is on"));
        assert!(list_error_message("poisoned").contains("Reopen Settings"));
        std::fs::remove_dir_all(&state.data_dir).ok();
    }
}
