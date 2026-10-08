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
//! Graph data (Phase 1 R11, the Tauri panel's Settings › Graph, `dist/panel.js`
//! `renderSettingsStats` / `settingsExport` / `settingsImportFile`):
//! - a stats Label exactly `<resources> link(s) · <links> connection(s)`, read from
//!   `ubilink_app::graph_stats` on every render, so it is re-read each time Settings is
//!   shown and right after an import.
//! - an `Export` Button: writes `PanelBackend::export_graph`'s JSON (core's
//!   `ubilink.graph.v3`) to the `export_path` fixture when a probe loaded one (O4), else
//!   to the path a save dialog returns (a release build; an automation build never
//!   opens a dialog on the host and says the fixture is missing instead).
//! - an `Import` Button: reads the `import_path` fixture (or an open dialog's file) and
//!   hands it to `ubilink_app::import_graph`, which merges it through the core's own
//!   validation policy (D16, S2): the shell adds no parsing or opener of its own.
//! - the hint line reports where the export went, the import summary, or what failed
//!   and what to do (U3).
//!
//! Reset (Phase 1 R12, the Tauri panel's `set-reset` / `set-reset-confirm`): the
//! `Reset…` Button deletes nothing by itself; it shows an inline confirmation — a Label
//! `Delete all captured links and connections?`, `This cannot be undone.`, and the
//! Buttons `Delete everything` and `Cancel` (Tauri wording, B1). Only `Delete
//! everything` calls `ubilink_app::reset_graph`; Cancel or Escape hides the question
//! and leaves the graph as it was. The confirmation is inline, not a gpui-component
//! `Dialog`, as in the Tauri panel (no Dialog role reaches the tree anyway,
//! A11Y-NOTES.md). When the confirmation closes, the button that had focus is gone, so
//! the view takes focus back and Tab starts from the top again.
//!
//! Launch at login (Phase 1 R13, spec D6): a `Switch` named `Launch at login` and a
//! status Label whose text is exactly `login_item::Status::label` (`Enabled`,
//! `Requires approval`, `Not registered`, `Not found`); requires-approval and
//! not-found also show the report's explanation verbatim in their own Label. Those
//! two are valid states, not failures (D6). The switch is on for Enabled and Requires
//! approval. Every status read, register and unregister asks the
//! [`crate::login_item`] seam first: an automation build always gets the
//! `login_item` fixture's answer (or an error saying none is loaded), so a probe
//! never touches the host's login items (O4); only a release build, where the seam
//! answers `None`, calls `SMAppService.mainApp` itself ([`sm_app_service`]). The
//! status is read each time Settings is shown (the panel builds a fresh view per
//! show) and replaced by the result of each register/unregister; a failed call says
//! what failed and what to do (U3).
//!
//! Keyboard: the view focuses itself when shown, so Tab (gpui-base `Root`'s
//! `focus_next`) reaches the first switch. The automation `key` verb only sends
//! key-down events, while GPUI's own Space/Enter-activates-a-focused-switch path fires
//! on key *up*; so each switch sits in a wrapper whose `capture_key_down` toggles it on
//! Space/Enter while the switch inside is focused, and calls `prevent_default` so the
//! switch's own key-up activation does not toggle it straight back for a real keyboard
//! user (the same pattern as `onboarding.rs`'s step buttons). The Export and Import
//! buttons use the same wrapper for Enter/Space.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use ubilink_app::PluginView;
use ubilink_core::GraphStats;

use super::Route;
use crate::fixtures::{self, Fixture};
use crate::login_item::{self, Report, Status};
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

/// The stats Label, worded as the Tauri settings view does (B1):
/// `5 links · 3 connections`, `1 link · 1 connection`, `0 links · 0 connections`.
pub fn stats_text(stats: &GraphStats) -> String {
    fn plural(n: i64) -> &'static str {
        if n == 1 {
            ""
        } else {
            "s"
        }
    }
    format!(
        "{} link{} · {} connection{}",
        stats.resources,
        plural(stats.resources),
        stats.links,
        plural(stats.links)
    )
}

/// The stats Label when the counts cannot be read (U3).
pub fn stats_error_message(e: &str) -> String {
    format!("Couldn't read the graph: {e}. Reopen Settings to try again.")
}

/// Write the whole graph as JSON (core's `ubilink.graph.v3` export) to exactly `path`,
/// returning the written path.
pub fn export_to(state: &ubilink_app::AppState, path: &Path) -> Result<String, String> {
    let json = {
        let backend = state
            .backend
            .lock()
            .map_err(|_| "internal state was poisoned".to_string())?;
        backend.export_graph()?
    };
    std::fs::write(path, json).map_err(|e| format!("couldn't write the export: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Read a graph-export JSON file at `path` and merge it into the graph through the
/// core's validation (`ubilink_app::import_graph`); returns its summary.
pub fn import_from(state: &ubilink_app::AppState, path: &Path) -> Result<String, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    ubilink_app::import_graph(state, &json)
}

/// The line shown after an export: where it went, or what failed and what to do (U3).
pub fn export_message(result: &Result<String, String>) -> (String, bool) {
    match result {
        Ok(path) => (format!("Exported to {path}"), false),
        Err(e) => (
            format!("Export failed: {e}. Check that the folder is writable, then try again."),
            true,
        ),
    }
}

/// The line shown after an import: core's summary, or what failed and what to do (U3).
pub fn import_message(result: &Result<String, String>) -> (String, bool) {
    match result {
        Ok(summary) => (summary.clone(), false),
        Err(e) => (
            format!("Import failed: {e}. Choose a ubilink export (.json) file, then try again."),
            true,
        ),
    }
}

/// The reset confirmation's question, its warning and its two buttons (Tauri wording).
pub const RESET_QUESTION: &str = "Delete all captured links and connections?";
pub const RESET_WARNING: &str = "This cannot be undone.";
pub const RESET_CONFIRM: &str = "Delete everything";
pub const RESET_CANCEL: &str = "Cancel";

/// Empty the graph (only ever called once the user confirmed); the line to show, and
/// whether it failed.
pub fn reset(state: &ubilink_app::AppState) -> (String, bool) {
    reset_message(&ubilink_app::reset_graph(state))
}

/// The line shown after a reset: how many links went, or what failed and what to do (U3).
pub fn reset_message(result: &Result<i64, String>) -> (String, bool) {
    match result {
        Ok(n) => (
            format!("Deleted {n} link{}.", if *n == 1 { "" } else { "s" }),
            false,
        ),
        Err(e) => (
            format!("Reset failed: {e}. Reopen Settings, then try again."),
            true,
        ),
    }
}

/// The launch-at-login switch's accessible name.
pub const LOGIN_SWITCH: &str = "Launch at login";

/// Release builds' explanations for the two statuses that need the user (U3); the
/// seam supplies its own (the fixture's) in automation builds.
pub const REQUIRES_APPROVAL_EXPLANATION: &str =
    "macOS needs your approval: turn on Ubilink in System Settings > General > Login Items.";
pub const NOT_FOUND_EXPLANATION: &str = "macOS cannot find this copy of Ubilink as a login item. Move Ubilink to Applications, open it from there and try again.";

/// The launch-at-login status: the seam's answer, else `SMAppService.mainApp`'s.
pub fn login_status() -> Result<Report, String> {
    login_item::status().unwrap_or_else(sm_app_service::status)
}

/// Turn launch at login on (register) or off (unregister) through the seam, else
/// `SMAppService.mainApp`; the resulting status.
pub fn set_launch_at_login(on: bool) -> Result<Report, String> {
    if on {
        login_item::register().unwrap_or_else(sm_app_service::register)
    } else {
        login_item::unregister().unwrap_or_else(sm_app_service::unregister)
    }
}

/// The line shown when reading the status fails (U3).
pub fn login_status_error_message(e: &str) -> String {
    format!("Couldn't read the launch-at-login status: {e}. Reopen Settings to try again.")
}

/// The line shown when turning launch at login on or off fails (U3).
pub fn login_toggle_error_message(on: bool, e: &str) -> String {
    format!(
        "Couldn't turn launch at login {}: {e}. Reopen Settings, then try again.",
        if on { "on" } else { "off" }
    )
}

/// `SMAppService.mainApp`, called only when the seam answers `None` (release builds).
mod sm_app_service {
    use super::{Report, Status, NOT_FOUND_EXPLANATION, REQUIRES_APPROVAL_EXPLANATION};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    fn report(status: Status) -> Report {
        let explanation = match status {
            Status::RequiresApproval => Some(REQUIRES_APPROVAL_EXPLANATION.to_string()),
            Status::NotFound => Some(NOT_FOUND_EXPLANATION.to_string()),
            _ => None,
        };
        Report {
            status,
            explanation,
        }
    }

    fn read() -> Status {
        // SAFETY: `mainAppService` and `status` take no arguments and are callable
        // from any thread on macOS 13+.
        let raw = unsafe { SMAppService::mainAppService().status() };
        match raw {
            SMAppServiceStatus::Enabled => Status::Enabled,
            SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
            SMAppServiceStatus::NotFound => Status::NotFound,
            _ => Status::NotRegistered,
        }
    }

    pub fn status() -> Result<Report, String> {
        Ok(report(read()))
    }

    /// After a call, the status macOS now reports. A register that macOS answers with
    /// an error while the status is requires-approval or not-found is that state (D6),
    /// shown with its explanation, not a failure.
    fn after(result: Result<(), String>) -> Result<Report, String> {
        let now = read();
        match result {
            Ok(()) => Ok(report(now)),
            Err(_) if matches!(now, Status::RequiresApproval | Status::NotFound) => Ok(report(now)),
            Err(e) => Err(e),
        }
    }

    pub fn register() -> Result<Report, String> {
        // SAFETY: as above; the error is an owned NSError.
        let result = unsafe { SMAppService::mainAppService().registerAndReturnError() };
        after(result.map_err(|e| e.localizedDescription().to_string()))
    }

    pub fn unregister() -> Result<Report, String> {
        // SAFETY: as above.
        let result = unsafe { SMAppService::mainAppService().unregisterAndReturnError() };
        after(result.map_err(|e| e.localizedDescription().to_string()))
    }
}

/// Where a release build's save dialog starts: Downloads (the Tauri app's export
/// folder), else home.
fn export_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let downloads = home.join("Downloads");
    if downloads.is_dir() {
        downloads
    } else {
        home
    }
}

/// The save dialog's suggested name, stamped like the Tauri app's export file.
fn export_file_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("ubilink-export-{stamp}.json")
}

/// An automation build never opens a real file dialog on the host (O4).
const NO_FIXTURE: &str = "no {kind} fixture loaded";

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
    /// Reset was pressed and the confirmation is showing; nothing is deleted yet.
    confirming_reset: bool,
    /// The open save/open dialog, if any; dropped (cancelled) with the view.
    _dialog: Task<()>,
    /// Launch at login: the status last read (on show) or reported by the last
    /// register/unregister, or the error line to show instead (U3).
    login: Result<Report, String>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        SettingsView {
            message: None,
            focus: cx.focus_handle(),
            focused: false,
            confirming_reset: false,
            _dialog: Task::ready(()),
            login: login_status().map_err(|e| login_status_error_message(&e)),
        }
    }

    /// Whether the launch-at-login switch shows on (off while the status is unknown).
    fn login_on(&self) -> bool {
        self.login.as_ref().is_ok_and(|r| r.status.is_on())
    }

    /// Register (`on`) or unregister, and show what macOS (or the seam) now reports.
    fn set_launch_at_login(&mut self, on: bool, cx: &mut Context<Self>) {
        self.login = set_launch_at_login(on).map_err(|e| login_toggle_error_message(on, &e));
        cx.notify();
    }

    /// The `Launch at login` switch, its status Label and, for requires-approval and
    /// not-found, the explanation Label (or the error line).
    fn login_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.login_on();
        let theme = cx.theme();
        let (status, explanation, failed): (Option<&'static str>, Option<String>, bool) =
            match &self.login {
                Ok(r) => (Some(r.status.label()), r.explanation.clone(), false),
                Err(e) => (None, Some(e.clone()), true),
            };
        let switch = div()
            .flex_shrink_0()
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if is_activation_key(&event.keystroke) {
                    this.set_launch_at_login(!on, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                Switch::new("launch-at-login")
                    .small()
                    .label(LOGIN_SWITCH)
                    .checked(on)
                    .on_click(
                        cx.listener(|this, next: &bool, _, cx| this.set_launch_at_login(*next, cx)),
                    ),
            );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::S2)
            .text_size(font_size::ROW)
            .child(switch)
            .when_some(status, |this, status| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_color(theme.foreground)
                        .child(text!(id = "settings-login-status", status)),
                )
            })
            .when_some(explanation, |this, explanation| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(font_size::META)
                        .text_color(if failed {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(text!(id = "settings-login-explanation", explanation)),
                )
            })
            .into_any_element()
    }

    fn show(&mut self, message: (String, bool), cx: &mut Context<Self>) {
        self.message = Some(message);
        cx.notify();
    }

    /// Export: to the `export_path` fixture if set, else where a save dialog says.
    fn export(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = fixtures::path(Fixture::ExportPath) {
            let result = export_to(crate::app_state::get(), &path);
            return self.show(export_message(&result), cx);
        }
        if cfg!(feature = "automation") {
            let e = NO_FIXTURE.replace("{kind}", "export_path");
            return self.show(export_message(&Err(e)), cx);
        }
        let chosen = cx.prompt_for_new_path(&export_dir(), Some(&export_file_name()));
        self._dialog = cx.spawn(async move |this, cx| {
            let result = match chosen.await {
                Ok(Ok(Some(path))) => export_to(crate::app_state::get(), &path),
                Ok(Ok(None)) => return,
                Ok(Err(e)) => Err(format!("couldn't open the save dialog: {e}")),
                Err(_) => return,
            };
            let _ = this.update(cx, |view, cx| view.show(export_message(&result), cx));
        });
    }

    /// Import: from the `import_path` fixture if set, else the file an open dialog picks.
    fn import(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = fixtures::path(Fixture::ImportPath) {
            let result = import_from(crate::app_state::get(), &path);
            return self.show(import_message(&result), cx);
        }
        if cfg!(feature = "automation") {
            let e = NO_FIXTURE.replace("{kind}", "import_path");
            return self.show(import_message(&Err(e)), cx);
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        self._dialog = cx.spawn(async move |this, cx| {
            let result = match chosen.await {
                Ok(Ok(Some(paths))) => match paths.first() {
                    Some(path) => import_from(crate::app_state::get(), path),
                    None => return,
                },
                Ok(Ok(None)) => return,
                Ok(Err(e)) => Err(format!("couldn't open the file dialog: {e}")),
                Err(_) => return,
            };
            let _ = this.update(cx, |view, cx| view.show(import_message(&result), cx));
        });
    }

    /// Reset…: only asks; the graph is untouched until `confirm_reset`.
    fn ask_reset(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.confirming_reset = true;
        self.message = None;
        cx.notify();
    }

    /// Hide the confirmation. Its focused button goes with it, so the view takes focus
    /// back: Tab keeps working from the top.
    fn close_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirming_reset = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Cancel: the graph stays exactly as it was.
    fn cancel_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_reset(window, cx);
    }

    /// Delete everything: the user confirmed, so empty the graph.
    fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirming_reset {
            return;
        }
        self.message = Some(reset(crate::app_state::get()));
        self.close_reset(window, cx);
    }

    /// A Button that Tab reaches and Enter/Space (or a click) activates; see the
    /// module doc for why the wrapper handles the keys.
    fn action_button(
        &self,
        id: &'static str,
        label: &'static str,
        danger: bool,
        action: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_shrink_0()
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if is_activation_key(&event.keystroke) {
                    action(this, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                Button::new(id)
                    .small()
                    .label(label)
                    .when(danger, |b| b.danger())
                    .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx))),
            )
            .into_any_element()
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
        let (stats, stats_failed) = match ubilink_app::graph_stats(crate::app_state::get()) {
            Ok(stats) => (stats_text(&stats), false),
            Err(e) => (stats_error_message(&e), true),
        };
        let export_button = self.action_button(
            "settings-export",
            "Export",
            false,
            |v, _, cx| v.export(cx),
            cx,
        );
        let import_button = self.action_button(
            "settings-import",
            "Import",
            false,
            |v, _, cx| v.import(cx),
            cx,
        );
        let reset_button =
            self.action_button("settings-reset", "Reset…", true, Self::ask_reset, cx);
        let confirmation = self.confirming_reset.then(|| {
            let confirm = self.action_button(
                "settings-reset-confirm",
                RESET_CONFIRM,
                true,
                Self::confirm_reset,
                cx,
            );
            let cancel = self.action_button(
                "settings-reset-cancel",
                RESET_CANCEL,
                false,
                Self::cancel_reset,
                cx,
            );
            (confirm, cancel)
        });
        let (hint, failed) = match (&list_error, &self.message) {
            (Some(e), _) => (e.clone(), true),
            (None, Some((m, failed))) => (m.clone(), *failed),
            (None, None) => (ADAPTERS_HINT.to_string(), false),
        };
        let login_row = self.login_row(cx);
        let theme = cx.theme();
        div()
            .track_focus(&self.focus)
            // Escape closes an open reset confirmation (Tauri) without deleting.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.confirming_reset
                    && event.keystroke.key == "escape"
                    && !event.keystroke.modifiers.modified()
                {
                    this.cancel_reset(window, cx);
                    cx.stop_propagation();
                }
            }))
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
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::S2)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(font_size::ROW)
                            .text_color(if stats_failed {
                                theme.danger
                            } else {
                                theme.foreground
                            })
                            .child(text!(id = "settings-stats", stats)),
                    )
                    .child(export_button)
                    .child(import_button)
                    .child(reset_button),
            )
            .when_some(confirmation, |this, (confirm, cancel)| {
                this.child(
                    div()
                        .id("settings-reset-confirmation")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::S2)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap_x(spacing::S1)
                                .text_size(font_size::ROW)
                                .child(
                                    div().text_color(theme.foreground).child(text!(
                                        id = "settings-reset-question",
                                        RESET_QUESTION
                                    )),
                                )
                                .child(
                                    div()
                                        .text_color(theme.danger)
                                        .child(text!(id = "settings-reset-warning", RESET_WARNING)),
                                ),
                        )
                        .child(confirm)
                        .child(cancel),
                )
            })
            .child(login_row)
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
    use super::{
        export_message, export_to, import_from, import_message, list_error_message,
        login_status_error_message, login_toggle_error_message, reset, reset_message, state,
        stats_error_message, stats_text, toggle, toggle_message, LOGIN_SWITCH,
        NOT_FOUND_EXPLANATION, REQUIRES_APPROVAL_EXPLANATION, RESET_CANCEL, RESET_CONFIRM,
        RESET_QUESTION,
    };
    use std::collections::HashSet;
    use std::sync::Mutex;
    use ubilink_app::{
        graph_stats, list_plugins, load_disabled, reset_graph, AppState, HotkeyReport,
    };
    use ubilink_core::{AdapterRegistry, GraphStats, LinkGraph, PanelBackend, UbilinkService};

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

    /// The graph-roundtrip probe's seed: 5 resources, 3 links.
    fn seed(state: &AppState) {
        let json = include_str!("../../probes/fixtures/graph-backlinks.json");
        ubilink_app::import_graph(state, json).unwrap();
    }

    #[test]
    fn settings_stats_label_matches_the_tauri_wording() {
        let s = |resources, links| stats_text(&GraphStats { resources, links });
        assert_eq!(s(5, 3), "5 links · 3 connections");
        assert_eq!(s(0, 0), "0 links · 0 connections");
        assert_eq!(s(1, 1), "1 link · 1 connection");
        assert!(stats_error_message("poisoned").contains("Reopen Settings"));
    }

    /// R11: export to a path, empty the graph, import that file: the counts are back.
    #[test]
    fn settings_graph_export_then_import_round_trips() {
        let state = temp_state("roundtrip");
        seed(&state);
        let before = graph_stats(&state).unwrap();
        assert_eq!((before.resources, before.links), (5, 3));

        let path = state.data_dir.join("roundtrip-export.json");
        let exported = export_to(&state, &path);
        let (msg, failed) = export_message(&exported);
        assert!(!failed && msg.contains("roundtrip-export.json"), "{msg}");
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["links"].as_array().unwrap().len(), 3);

        reset_graph(&state).unwrap();
        assert_eq!(graph_stats(&state).unwrap().links, 0);

        let (msg, failed) = import_message(&import_from(&state, &path));
        assert!(!failed, "{msg}");
        let after = graph_stats(&state).unwrap();
        assert_eq!(
            (after.resources, after.links),
            (before.resources, before.links)
        );
        std::fs::remove_dir_all(&state.data_dir).ok();
    }

    /// Failures say what failed and what to do (U3).
    #[test]
    fn settings_graph_failures_say_what_to_do() {
        let state = temp_state("graphfail");
        let missing = state.data_dir.join("nope.json");
        let (msg, failed) = import_message(&import_from(&state, &missing));
        assert!(
            failed && msg.starts_with("Import failed") && msg.contains("try again"),
            "{msg}"
        );
        let bad = state.data_dir.join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let (msg, failed) = import_message(&import_from(&state, &bad));
        assert!(failed && msg.contains("try again"), "{msg}");
        let unwritable = state.data_dir.join("no-such-dir").join("x.json");
        let (msg, failed) = export_message(&export_to(&state, &unwritable));
        assert!(
            failed && msg.starts_with("Export failed") && msg.contains("try again"),
            "{msg}"
        );
        std::fs::remove_dir_all(&state.data_dir).ok();
    }

    /// R12: confirming empties the graph, and the result line says how much went.
    #[test]
    fn settings_reset_empties_the_graph_once_confirmed() {
        let state = temp_state("reset");
        seed(&state);
        let before = graph_stats(&state).unwrap();
        assert_eq!((before.resources, before.links), (5, 3));

        let (msg, failed) = reset(&state);
        assert!(!failed && msg.starts_with("Deleted "), "{msg}");
        let after = graph_stats(&state).unwrap();
        assert_eq!((after.resources, after.links), (0, 0));
        assert_eq!(stats_text(&after), "0 links · 0 connections");
        std::fs::remove_dir_all(&state.data_dir).ok();
    }

    /// The confirmation's wording is the Tauri panel's, and failures say what to do (U3).
    #[test]
    fn settings_reset_wording_and_failures() {
        assert_eq!(RESET_QUESTION, "Delete all captured links and connections?");
        assert_eq!(RESET_CONFIRM, "Delete everything");
        assert_eq!(RESET_CANCEL, "Cancel");
        assert_eq!(reset_message(&Ok(1)).0, "Deleted 1 link.");
        assert_eq!(reset_message(&Ok(5)).0, "Deleted 5 links.");
        let (msg, failed) = reset_message(&Err("internal state was poisoned".into()));
        assert!(
            failed && msg.starts_with("Reset failed") && msg.contains("try again"),
            "{msg}"
        );
    }

    /// R13: the switch's name, the release explanations (the fixtures' wording) and
    /// the failure lines, which say what failed and what to do (U3).
    #[test]
    fn launch_at_login_wording() {
        assert_eq!(LOGIN_SWITCH, "Launch at login");
        let fixture = |name: &str| -> String {
            let path = format!("{}/probes/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            doc["explanation"].as_str().unwrap().to_string()
        };
        assert_eq!(
            REQUIRES_APPROVAL_EXPLANATION,
            fixture("login-item-requires-approval.json")
        );
        assert_eq!(NOT_FOUND_EXPLANATION, fixture("login-item-not-found.json"));
        let msg = login_toggle_error_message(true, "denied");
        assert!(msg.starts_with("Couldn't turn launch at login on") && msg.contains("try again"));
        assert!(login_status_error_message("x").contains("Reopen Settings"));
    }

    /// I9/O4: an automation build with no `login_item` fixture reports an error the view
    /// shows, never a status from the host's real SMAppService.
    #[cfg(feature = "automation")]
    #[test]
    fn launch_at_login_without_a_fixture_is_an_error() {
        let expected = Err(crate::login_item::NO_FIXTURE.to_string());
        assert_eq!(super::login_status(), expected);
        assert_eq!(super::set_launch_at_login(true), expected);
        assert_eq!(super::set_launch_at_login(false), expected);
    }
}
