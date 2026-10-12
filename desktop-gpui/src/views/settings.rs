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
//! Layout (rc.6, D26 amendment): an Apple Notes-style settings form. Four groups,
//! split by hairlines, each a form row (`theming::form_row`): the group's label
//! (`Permissions:`, `Data:`, `Login:`, `App adapters:`) right-aligned in the label
//! column, its controls from one left edge, descriptions under them. Every button is a
//! grey capsule (Delete everything a red one): nothing here is the panel's one default
//! action, so nothing is accent-filled.
//!
//! Permissions (rc.4, the Tauri panel's Settings › Permissions, `dist/index.html`
//! `set-auto` / `set-open-perms`), the first group, labelled `Permissions`: an
//! `Automation` row with the purpose `Read the active
//! app's link.` and a status Label, exactly `Allowed`, `Not allowed` or `Not checked
//! yet` ([`automation_status_text`]); and an `Open permission setup…` Button that
//! switches the panel to onboarding. The status comes from the
//! check onboarding uses (`onboarding::check_permissions`: the `permission` fixture,
//! or a release build's live probe), run off the main thread each time Settings is
//! shown, so it never blocks a frame; it reads `Checking…` until it answers.
//!
//! Keyboard: the view focuses itself when shown, so Tab (gpui-base `Root`'s
//! `focus_next`) reaches the first switch. The automation `key` verb only sends
//! key-down events, while GPUI's own Space/Enter-activates-a-focused-switch path fires
//! on key *up*; so each switch sits in a wrapper whose `capture_key_down` toggles it on
//! Space/Enter while the switch inside is focused, and calls `prevent_default` so the
//! switch's own key-up activation does not toggle it straight back for a real keyboard
//! user (the same pattern as `onboarding.rs`'s step buttons). The Export and Import
//! buttons use the same wrapper for Enter/Space.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use gpui_kit::component::{ActiveTheme, Disableable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use ubilink_app::PluginView;
use ubilink_core::GraphStats;

use super::escape::{self, EscapeAction, EscapeState};
use super::layout::Layout;
use super::onboarding::{self, Permission};
use super::theming::{self, FollowAppearance, Sample, Samples, ThemeSamples};
use super::Route;
use crate::fixtures::{self, Fixture};
use crate::login_item::{self, Report, Status};
use crate::theme::tokens::{font_size, font_weight, radius, size, spacing};

/// The settings route's view-state, for Task 6's automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
    /// Where the view's opaque surface, primary text and accent control are (R14,
    /// `views::theming`).
    #[serde(default)]
    pub theme_samples: ThemeSamples,
    /// Measured places and text sizes (`views::layout`): `viewport`, the adapter
    /// grid's scroll box (`adapters_box`) and its last switch (`last_adapter`),
    /// the action buttons (`export_button`, `import_button`, `reset_button`, and while
    /// the reset question shows `reset_confirm_button`, `reset_cancel_button`), and the
    /// launch-at-login `login_label` / `login_status` text sizes.
    #[serde(default)]
    pub layout: BTreeMap<String, serde_json::Value>,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Settings(State {
        view: Route::Settings.as_str().into(),
        phase: "ready".into(),
        theme_samples: SAMPLES.get(),
        layout: LAYOUT.get(),
    })
}

/// The newest settings view's theme sample regions (R14), written at prepaint.
static SAMPLES: Samples = Samples::new();

/// The newest settings view's measured layout (`views::layout`), written at prepaint.
static LAYOUT: Layout = Layout::new();

/// How an action button looks (rc.6, a Notes-style settings form: capsules, and no
/// accent-filled button, since nothing here is the one default action).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// A plain grey capsule (Open permission setup…, Export, Import, Reset…, Cancel).
    Plain,
    /// The confirmed destructive action: a red capsule (Delete everything).
    Destructive,
}

// Settings' layout (rc.6, a Notes-style form), in points. The view sets these on its
// rows, so `content_height` adds up exactly what the view lays out, and the window fits
// the content (the Tauri panel's `fitWindow`) instead of clipping the adapter grid.
const PAD_X: Pixels = spacing::S5;
const PAD_Y: Pixels = spacing::S3;
const GAP: Pixels = spacing::S1;
/// The title row: a 13 pt headline's line box, at the control height.
const HEADER_H: Pixels = size::CONTROL;
/// Above and below each group's form row; groups are split by a hairline.
const GROUP_PAD_Y: Pixels = spacing::S2;
/// The groups: Permissions, Data, Login, App adapters.
const GROUPS: usize = 4;
/// A description line under a control (meta size, secondary colour).
const DESCRIPTION_LINE_H: Pixels = px(16.);
/// The reset question's row: two 18 pt lines beside its buttons.
const CONFIRM_H: Pixels = px(40.);
const CONFIRM_LINE_H: Pixels = px(18.);
/// The launch-at-login line, and with its explanation under it: up to three 16 pt
/// lines (the not-found explanation wraps to three in the control column).
const LOGIN_H: Pixels = px(20.);
const LOGIN_EXPLAINED_H: Pixels = px(68.);
const EXPLANATION_LINE_H: Pixels = DESCRIPTION_LINE_H;
/// One adapter switch, and the grid's gap between them.
const ADAPTER_W: Pixels = px(140.);
const ADAPTER_H: Pixels = px(16.);
const ADAPTER_GAP: Pixels = spacing::S2;
const HINT_H: Pixels = px(18.);

/// What Settings' height depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Content {
    /// How many adapter switches the grid holds.
    pub adapters: usize,
    /// The reset question's row is showing.
    pub confirming_reset: bool,
    /// The launch-at-login row carries an explanation (or an error) line.
    pub login_explained: bool,
}

/// How many adapter switches fit on one grid row of a `panel_w` wide panel: the
/// control column, right of the label column.
pub fn adapter_columns(panel_w: f64) -> usize {
    let pt = |p: Pixels| f64::from(f32::from(p));
    let inner = panel_w - 2.0 * pt(PAD_X) - pt(size::FORM_LABEL) - pt(theming::FORM_GAP);
    (((inner + pt(ADAPTER_GAP)) / (pt(ADAPTER_W) + pt(ADAPTER_GAP))).floor() as usize).max(1)
}

/// Settings' panel height for `content`: padding, the title, then the four groups
/// (each its form row with padding above and below, a hairline between groups):
/// Permissions (the Automation line and its description), Data (stats and actions,
/// and the reset question when it shows), Login (the switch, and its explanation) and
/// App adapters (every grid row and the hint line). The shell clamps it (96..900) and
/// cuts it to the display, past which the adapter grid scrolls inside.
pub fn content_height(content: Content) -> f64 {
    let pt = |p: Pixels| f64::from(f32::from(p));
    let cols = adapter_columns(crate::shell::panel::CARD_WIDTH);
    let rows = content.adapters.div_ceil(cols);
    let grid = if rows == 0 {
        0.0
    } else {
        rows as f64 * pt(ADAPTER_H) + (rows - 1) as f64 * pt(ADAPTER_GAP)
    };
    let permissions = pt(theming::CONTROL_H) + pt(DESCRIPTION_LINE_H);
    let mut data = pt(theming::CONTROL_H);
    if content.confirming_reset {
        data += pt(GAP) + pt(CONFIRM_H);
    }
    let login = pt(if content.login_explained {
        LOGIN_EXPLAINED_H
    } else {
        LOGIN_H
    });
    let adapters = grid + pt(GAP) + pt(HINT_H);
    let groups = GROUPS as f64 * 2.0 * pt(GROUP_PAD_Y)
        + (GROUPS - 1) as f64 * pt(theming::HAIRLINE)
        + permissions
        + data
        + login
        + adapters;
    2.0 * pt(PAD_Y) + pt(HEADER_H) + groups
}

/// This route's row in the router table (`views::TABLE`): the height a freshly shown
/// Settings needs (no reset question yet).
pub fn panel_height() -> f64 {
    let adapters = ubilink_app::list_plugins(crate::app_state::get()).map_or(0, |p| p.len());
    content_height(Content {
        adapters,
        confirming_reset: false,
        login_explained: login_explained(&login_status()),
    })
}

/// Whether the launch-at-login row shows its explanation (or error) line, up to three lines.
fn login_explained(login: &Result<Report, String>) -> bool {
    login.as_ref().map_or(true, |r| r.explanation.is_some())
}

/// The Permissions section (rc.4): its heading, the Automation row's name and purpose
/// (Tauri wording), the status texts, and the button that opens onboarding.
pub const PERMISSIONS_HEADING: &str = "Permissions";
/// The other groups' form labels (rc.6; the colon is drawn after each).
pub const DATA_LABEL: &str = "Data";
pub const LOGIN_LABEL: &str = "Login";
pub const ADAPTERS_LABEL: &str = "App adapters";
pub const AUTOMATION_PURPOSE: &str = "Read the active app's link.";
pub const AUTOMATION_ALLOWED: &str = "Allowed";
pub const AUTOMATION_NOT_ALLOWED: &str = "Not allowed";
pub const AUTOMATION_NOT_CHECKED: &str = "Not checked yet";
pub const AUTOMATION_CHECKING: &str = "Checking…";
pub const OPEN_PERMISSION_SETUP: &str = "Open permission setup…";

/// The Automation status Label for a check: `None` while it runs; a report says
/// `Allowed` (granted) or `Not allowed` (denied, or never asked: the Tauri pill's
/// `automation === 'granted'`); a check that could not decide says `Not checked yet`.
pub fn automation_status_text(check: Option<&Result<onboarding::Report, String>>) -> &'static str {
    match check {
        None => AUTOMATION_CHECKING,
        Some(Ok(report)) if report.granted(Permission::Automation) => AUTOMATION_ALLOWED,
        Some(Ok(_)) => AUTOMATION_NOT_ALLOWED,
        Some(Err(_)) => AUTOMATION_NOT_CHECKED,
    }
}

const ADAPTERS_HINT: &str =
    "Turn off an app to stop capturing from it. Built-in apps are always on.";

/// The line shown after a toggle: what changed, or what failed and what to do (U3).
pub fn toggle_message(app: &str, enabled: bool, result: &Result<(), String>) -> String {
    match result {
        Ok(()) if enabled => format!("{app} is on: Ubilink captures links from it again."),
        Ok(()) => format!("{app} is off: Ubilink no longer captures links from it."),
        Err(e) => format!(
            "Couldn't turn {app} {}: {e} Check that Ubilink can write its data folder, then try again.",
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

/// The stats Label, worded as the Tauri settings view was (B1):
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

/// The version Label, `Ubilink <version>` (B1: the Tauri panel's Settings footer,
/// `renderVersion`), so a bug report can name the build. The crate version is the app
/// version (scripts/version.sh reads this crate's Cargo.toml), and the RC checklist in
/// docs/RUNBOOK-release.md checks it on the released app.
pub fn version_text() -> String {
    format!("Ubilink {}", env!("CARGO_PKG_VERSION"))
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
            format!("Import failed: {e}. Choose a Ubilink export (.json) file, then try again."),
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

/// Where a release build's Export writes, as the Tauri app's did: `home`'s Downloads
/// folder, else `home` itself.
pub fn export_dir_in(home: &Path) -> PathBuf {
    let downloads = home.join("Downloads");
    if downloads.is_dir() {
        downloads
    } else {
        home.to_path_buf()
    }
}

/// The user's export folder ([`export_dir_in`] of `$HOME`), or why there is none.
fn export_dir() -> Result<PathBuf, String> {
    home_from(std::env::var_os("HOME")).map(|home| export_dir_in(&home))
}

/// `$HOME` as an absolute folder. Unset, empty or relative is refused: writing an
/// export relative to wherever the app happened to start would lose it.
pub fn home_from(home: Option<OsString>) -> Result<PathBuf, String> {
    match home.map(PathBuf::from) {
        Some(h) if h.is_absolute() => Ok(h),
        _ => Err("couldn't find your home folder ($HOME is not set to an absolute path)".into()),
    }
}

/// The command that shows `path` selected in a Finder window (the Tauri app's
/// `open -R <path>` after an export): a program and its argv, never a shell line, so
/// the path is one argument whatever it contains.
pub fn reveal_command(path: &Path) -> (&'static str, Vec<OsString>) {
    (
        "/usr/bin/open",
        vec![OsString::from("-R"), path.as_os_str().into()],
    )
}

/// An automation build never opens a real file dialog on the host (O4).
const NO_FIXTURE: &str = "no {kind} fixture loaded";

/// The `layout` key an action button's measured bounds are reported under.
fn layout_key(id: &'static str) -> &'static str {
    match id {
        "settings-export" => "export_button",
        "settings-import" => "import_button",
        "settings-reset" => "reset_button",
        "settings-reset-confirm" => "reset_confirm_button",
        "settings-reset-cancel" => "reset_cancel_button",
        "settings-open-permissions" => "permissions_button",
        other => other,
    }
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
    /// Reset was pressed and the confirmation is showing; nothing is deleted yet.
    confirming_reset: bool,
    /// The open file dialog's task, if any; dropped (cancelled) with the view.
    _dialog: Task<()>,
    /// Launch at login: the status last read (on show) or reported by the last
    /// register/unregister, or the error line to show instead (U3).
    login: Result<Report, String>,
    /// Repaints in the system's light or dark tokens while open (R14).
    appearance: FollowAppearance,
    /// The content the panel was last fitted to; a change refits it.
    fitted: Option<Content>,
    /// The Automation check's answer (rc.4); `None` while it runs.
    automation: Option<Result<onboarding::Report, String>>,
    /// The in-flight permission check; dropped (cancelled) with the view.
    _permission_check: Task<()>,
    /// The adapter grid's scroll position (its box scrolls when the panel is short).
    adapters_scroll: ScrollHandle,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // A fresh view per show, so the status is re-read each time Settings opens,
        // off the main thread (the release probe runs osascript).
        let permission_check = cx.spawn(async move |this, cx| {
            let check = cx
                .background_executor()
                .spawn(async { onboarding::check_permissions() })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.automation = Some(check);
                cx.notify();
            });
        });
        SettingsView {
            message: None,
            focus: cx.focus_handle(),
            focused: false,
            confirming_reset: false,
            _dialog: Task::ready(()),
            login: login_status().map_err(|e| login_status_error_message(&e)),
            appearance: FollowAppearance::default(),
            fitted: None,
            automation: None,
            _permission_check: permission_check,
            adapters_scroll: ScrollHandle::new(),
        }
    }

    /// Open permission setup…: the same panel switches to onboarding (the Tauri
    /// panel's `openSetup`), once this click's dispatch is over.
    fn open_permission_setup(&mut self, cx: &mut Context<Self>) {
        cx.defer(|cx| crate::shell::execute(crate::shell::Command::Open(Route::Onboarding), cx));
    }

    /// A group's form label (`Permissions:`, `Data:`, `Login:`, `App adapters:`),
    /// right-aligned in the label column; its laid-out text is measured as
    /// `layout.<key>` so a probe can check every label ends on one edge.
    fn group_label(
        id: &'static str,
        text: &'static str,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        theming::form_label(id, text, cx).child(LAYOUT.bounds(key))
    }

    /// The Permissions group's control (rc.4; rc.6 form): the Automation name and its
    /// status, then the button that opens onboarding, on one line; the purpose under
    /// them, in the meta size and secondary colour.
    fn permissions_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = self.action_button(
            "settings-open-permissions",
            OPEN_PERMISSION_SETUP,
            Look::Plain,
            |v, _, cx| v.open_permission_setup(cx),
            cx,
        );
        let status = automation_status_text(self.automation.as_ref());
        let foreground = theming::primary_text(cx);
        let theme = cx.theme();
        let status_color = match status {
            AUTOMATION_ALLOWED => theme.success,
            AUTOMATION_NOT_ALLOWED => theme.danger,
            _ => theme.muted_foreground,
        };
        div()
            .id("settings-permissions")
            .flex()
            .flex_col()
            .child(
                div()
                    .h(theming::CONTROL_H)
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::S2)
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(font_size::ROW)
                            .text_color(foreground)
                            .relative()
                            .child(text!(
                                id = "settings-automation-name",
                                Permission::Automation.name()
                            ))
                            .child(LAYOUT.bounds("automation_name")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(font_size::META)
                            .text_color(status_color)
                            .child(text!(id = "settings-automation-status", status)),
                    )
                    .child(button),
            )
            .child(
                div()
                    .h(DESCRIPTION_LINE_H)
                    .line_height(DESCRIPTION_LINE_H)
                    .truncate()
                    .text_size(font_size::META)
                    .text_color(theme.muted_foreground)
                    .child(text!(
                        id = "settings-automation-purpose",
                        AUTOMATION_PURPOSE
                    )),
            )
            .into_any_element()
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
        let foreground = theming::primary_text(cx);
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
                // The visible label is the view's own text (below), so its size is the
                // view's to choose and to measure; the switch keeps the accessible name.
                theming::switch("launch-at-login")
                    .accessibility_label(LOGIN_SWITCH)
                    .checked(on)
                    .on_click(
                        cx.listener(|this, next: &bool, _, cx| this.set_launch_at_login(*next, cx)),
                    ),
            );
        let label = div()
            .id("launch-at-login-label")
            .relative()
            .flex_shrink_0()
            .text_color(foreground)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.set_launch_at_login(!on, cx)))
            .child(LOGIN_SWITCH)
            .child(LAYOUT.font("login_label"));
        let explained = login_explained(&self.login);
        div()
            .flex_shrink_0()
            .min_h(if explained {
                LOGIN_EXPLAINED_H
            } else {
                LOGIN_H
            })
            .flex()
            .flex_col()
            // The label, its status and the explanation share one size: the Tauri
            // settings' meta size (`.set-pill`, `.set-sub`: --fs-meta).
            .text_size(font_size::META)
            .child(
                div()
                    .h(LOGIN_H)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::S2)
                    .child(switch)
                    .child(label)
                    .when_some(status, |this, status| {
                        this.child(
                            div()
                                .relative()
                                .flex_shrink_0()
                                .text_color(foreground)
                                .child(text!(id = "settings-login-status", status))
                                .child(LAYOUT.font("login_status")),
                        )
                    }),
            )
            // Under the control, on its text's left edge (past the switch).
            .when_some(explanation, |this, explanation| {
                this.child(
                    div()
                        .min_w_0()
                        .text_size(font_size::META)
                        .line_height(EXPLANATION_LINE_H)
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

    /// Export: to the `export_path` fixture if set; else, as the Tauri app did, no
    /// dialog: `ubilink_app::export_graph` writes a stamped file into Downloads (or
    /// home), Finder reveals it, and the hint line names the path.
    fn export(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = fixtures::path(Fixture::ExportPath) {
            let result = export_to(crate::app_state::get(), &path);
            return self.show(export_message(&result), cx);
        }
        if cfg!(feature = "automation") {
            // O4: an automation build never writes to the host's Downloads or opens
            // Finder; a probe loads an `export_path` fixture.
            let e = NO_FIXTURE.replace("{kind}", "export_path");
            return self.show(export_message(&Err(e)), cx);
        }
        let result =
            export_dir().and_then(|dir| ubilink_app::export_graph(crate::app_state::get(), &dir));
        if let Ok(path) = &result {
            let (program, args) = reveal_command(Path::new(path));
            // Best effort, as Tauri's: the file is written whether or not Finder opens.
            // `open` returns at once; reap it off the main thread so no zombie stays.
            match std::process::Command::new(program).args(args).spawn() {
                Ok(mut child) => {
                    std::thread::spawn(move || child.wait());
                }
                Err(e) => eprintln!("[ubilink] couldn't reveal the export in Finder: {e}"),
            }
        }
        self.show(export_message(&result), cx);
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
        // The shell fronts the dialog (activates the app, lowers the panel) and
        // refuses a second one while it is up; see `shell::begin_file_dialog`.
        if !crate::shell::begin_file_dialog(cx) {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        self._dialog = cx.spawn(async move |this, cx| {
            let answer = chosen.await;
            // Every way out (a file, Cancel or Escape, an error) restores the panel.
            // If this view goes away first (a route switch, a hide), the shell has
            // already closed the dialog and restored the panel, and this task is gone.
            cx.update(|cx| crate::shell::end_file_dialog(true, cx));
            let result = match answer {
                Ok(Ok(Some(paths))) => paths
                    .first()
                    .map(|path| import_from(crate::app_state::get(), path)),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(e)) => Some(Err(format!("couldn't open the file dialog: {e}"))),
            };
            if let Some(result) = result {
                let _ = this.update(cx, |view, cx| view.show(import_message(&result), cx));
            }
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
        look: Look,
        action: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .relative()
            .flex_shrink_0()
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if is_activation_key(&event.keystroke) {
                    action(this, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                // One height for every action button (macOS's 24 pt push button, as
                // the Tauri panel's `.set-btn` were one height), all capsules (rc.6).
                match look {
                    Look::Plain => theming::push_capsule_button(id),
                    Look::Destructive => theming::destructive_capsule_button(id),
                }
                .label(label)
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx))),
            )
            .child(LAYOUT.bounds(layout_key(id)))
            .into_any_element()
    }

    fn set_enabled(&mut self, app: &str, enabled: bool, cx: &mut Context<Self>) {
        self.message = Some(toggle(crate::app_state::get(), app, enabled));
        cx.notify();
    }

    /// `last`: the grid's last switch, whose place the layout reports (`last_adapter`).
    /// `sample`: this checked switch is the view's R14/D26 accent sample.
    fn adapter_switch(
        &self,
        plugin: &PluginView,
        last: bool,
        sample: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app: SharedString = plugin.app.clone().into();
        let enabled = plugin.enabled;
        let locked = plugin.builtin;
        let key_app = app.clone();
        let click_app = app.clone();
        div()
            .relative()
            .w(ADAPTER_W)
            .h(ADAPTER_H)
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
                theming::switch(switch_id(&app))
                    .label(app.clone())
                    .checked(enabled)
                    .disabled(locked)
                    .when(locked, |s| s.tooltip("Built-in: always on"))
                    .on_click(cx.listener(move |this, next: &bool, _, cx| {
                        this.set_enabled(&click_app, *next, cx)
                    })),
            )
            .when(sample, |d| d.child(SAMPLES.probe(Sample::Accent)))
            .when(last, |d| d.child(LAYOUT.bounds("last_adapter")))
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        SAMPLES.clear();
        LAYOUT.clear();
        self.appearance.ensure(window, cx);
        let (switches, list_error) = match ubilink_app::list_plugins(crate::app_state::get()) {
            Ok(plugins) => {
                // The accent sample: the first switch that is on and togglable (a
                // built-in one is drawn disabled, at half strength).
                let sample = plugins.iter().position(|p| p.enabled && !p.builtin);
                (
                    plugins
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            self.adapter_switch(p, i + 1 == plugins.len(), sample == Some(i), cx)
                        })
                        .collect::<Vec<_>>(),
                    None,
                )
            }
            Err(e) => (Vec::new(), Some(list_error_message(&e))),
        };
        let (stats, stats_failed) = match ubilink_app::graph_stats(crate::app_state::get()) {
            Ok(stats) => (stats_text(&stats), false),
            Err(e) => (stats_error_message(&e), true),
        };
        let export_button = self.action_button(
            "settings-export",
            "Export",
            Look::Plain,
            |v, _, cx| v.export(cx),
            cx,
        );
        let import_button = self.action_button(
            "settings-import",
            "Import",
            Look::Plain,
            |v, _, cx| v.import(cx),
            cx,
        );
        let reset_button = self.action_button(
            "settings-reset",
            "Reset…",
            // A push button, as macOS's: it only asks; the confirmation's Delete
            // everything is the destructive (red) one.
            Look::Plain,
            Self::ask_reset,
            cx,
        );
        let confirmation = self.confirming_reset.then(|| {
            let confirm = self.action_button(
                "settings-reset-confirm",
                RESET_CONFIRM,
                Look::Destructive,
                Self::confirm_reset,
                cx,
            );
            let cancel = self.action_button(
                "settings-reset-cancel",
                RESET_CANCEL,
                Look::Plain,
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
        let content = Content {
            adapters: switches.len(),
            confirming_reset: self.confirming_reset,
            login_explained: login_explained(&self.login),
        };
        if self.fitted != Some(content) {
            // Content changed since the panel was sized (the reset question opened or
            // closed, the login status gained an explanation): refit, after this frame.
            self.fitted = Some(content);
            let height = content_height(content);
            cx.defer(move |cx| crate::shell::fit_panel(height, cx));
        }
        let login_control = self.login_row(cx);
        let permissions_control = self.permissions_control(cx);
        let permissions_label = Self::group_label(
            "settings-permissions-heading",
            PERMISSIONS_HEADING,
            "label_permissions",
            cx,
        );
        let data_label = Self::group_label("settings-data-label", DATA_LABEL, "label_data", cx);
        let login_label = Self::group_label("settings-login-group", LOGIN_LABEL, "label_login", cx);
        let adapters_label = Self::group_label(
            "settings-adapters-heading",
            ADAPTERS_LABEL,
            "label_adapters",
            cx,
        );
        let surface = theming::surface(cx);
        let foreground = theming::primary_text(cx);
        let theme = cx.theme();
        let hairline = theme.border;
        // One group of the form: its row, padded, with a hairline above all but the
        // first (Notes' settings: hairlines, not section headings).
        let group = |first: bool, row: Div| {
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .py(GROUP_PAD_Y)
                .when(!first, |d| {
                    d.border_t(theming::HAIRLINE).border_color(hairline)
                })
                .child(row)
        };
        let data_control =
            div()
                .flex()
                .flex_col()
                .gap(GAP)
                .child(
                    div()
                        .h(theming::CONTROL_H)
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::S2)
                        .child(
                            div()
                                .flex_shrink(1.)
                                .min_w_0()
                                .truncate()
                                .text_size(font_size::ROW)
                                .text_color(if stats_failed {
                                    theme.danger
                                } else {
                                    foreground
                                })
                                .relative()
                                .child(text!(id = "settings-stats", stats))
                                .child(LAYOUT.bounds("stats")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(theming::SAMPLE_MIN + 8.))
                                .h(theming::CONTROL_H)
                                .relative()
                                .child(SAMPLES.probe(Sample::Surface)),
                        )
                        .child(export_button)
                        .child(import_button)
                        .child(reset_button),
                )
                .when_some(confirmation, |this, (confirm, cancel)| {
                    this.child(
                        div()
                            .id("settings-reset-confirmation")
                            .flex_shrink_0()
                            .min_h(CONFIRM_H)
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
                                    .line_height(CONFIRM_LINE_H)
                                    .child(div().text_color(foreground).child(text!(
                                        id = "settings-reset-question",
                                        RESET_QUESTION
                                    )))
                                    .child(div().text_color(theme.danger).child(text!(
                                        id = "settings-reset-warning",
                                        RESET_WARNING
                                    ))),
                            )
                            .child(confirm)
                            .child(cancel),
                    )
                });
        // Bounded by the chain group -> form row -> control column -> this -> the box
        // (each flex_1 / min_h_0), so when the panel is cut short (the shell's 900 pt
        // clamp, a short display) the grid scrolls inside its box and the footnote
        // stays in view.
        let adapters_control = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(GAP)
            .child(
                // The grid's scroll box, wrapped so its own place can be measured (a
                // canvas inside it would scroll with the switches).
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("settings-adapters")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.adapters_scroll)
                            .child(
                                // The grid itself, measured as `adapters_content`: its
                                // height is what the box scrolls through.
                                div()
                                    .relative()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .content_start()
                                    .gap_x(ADAPTER_GAP)
                                    .gap_y(ADAPTER_GAP)
                                    .children(switches)
                                    .child(LAYOUT.bounds("adapters_content")),
                            ),
                    )
                    .child(LAYOUT.bounds("adapters_box"))
                    .child(LAYOUT.scroll("adapters_scroll", &self.adapters_scroll)),
            )
            // The footnote under the grid, on the control column's edge.
            .child(
                div()
                    .relative()
                    .h(HINT_H)
                    .flex_shrink_0()
                    .line_height(HINT_H)
                    .truncate()
                    .text_size(font_size::META)
                    .text_color(if failed {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    })
                    .child(text!(id = "settings-hint", hint))
                    .child(LAYOUT.bounds("hint")),
            );
        div()
            .track_focus(&self.focus)
            // Escape closes an open reset confirmation without deleting, else
            // dismisses the panel (Tauri, `views::escape`).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !escape::is_escape(&event.keystroke) {
                    return;
                }
                let state = EscapeState {
                    confirming_reset: this.confirming_reset,
                    ..EscapeState::default()
                };
                match escape::escape_action(Route::Settings, state) {
                    EscapeAction::CloseConfirmation => this.cancel_reset(window, cx),
                    _ => escape::dismiss_panel(window, cx),
                }
                cx.stop_propagation();
            }))
            .size_full()
            .flex()
            .flex_col()
            .px(PAD_X)
            .py(PAD_Y)
            .rounded(radius::CARD)
            // The opaque card (R14, D26), inside the panel's glass frame.
            .bg(surface)
            .child(
                div()
                    .h(HEADER_H)
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(spacing::S3)
                    .child(
                        div()
                            .relative()
                            .text_size(font_size::TITLE)
                            .font_weight(font_weight::HEADLINE)
                            .text_color(foreground)
                            .child(text!(Route::Settings.title()))
                            .child(SAMPLES.probe(Sample::Text)),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_size(font_size::META)
                            .text_color(theme.muted_foreground)
                            .child(text!(id = "settings-version", version_text())),
                    ),
            )
            .child(group(
                true,
                theming::form_row(permissions_label, theming::CONTROL_H, permissions_control),
            ))
            .child(group(
                false,
                theming::form_row(data_label, theming::CONTROL_H, data_control),
            ))
            .child(group(
                false,
                theming::form_row(login_label, LOGIN_H, login_control),
            ))
            .child(
                group(
                    false,
                    theming::form_row(adapters_label, ADAPTER_H, adapters_control)
                        .flex_1()
                        .min_h_0(),
                )
                .flex_1()
                .min_h_0(),
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
        adapter_columns, automation_status_text, content_height, export_dir_in, export_message,
        export_to, home_from, import_from, import_message, list_error_message,
        login_status_error_message, login_toggle_error_message, reset, reset_message,
        reveal_command, state, stats_error_message, stats_text, toggle, toggle_message,
        version_text, Content, LOGIN_SWITCH, NOT_FOUND_EXPLANATION, REQUIRES_APPROVAL_EXPLANATION,
        RESET_CANCEL, RESET_CONFIRM, RESET_QUESTION,
    };
    use crate::views::onboarding::Report;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use ubilink_app::{
        graph_stats, list_plugins, load_disabled, reset_graph, AppState, HotkeyReport,
    };
    use ubilink_core::{AdapterRegistry, GraphStats, LinkGraph, PanelBackend, UbilinkService};

    fn temp_state(tag: &str) -> AppState {
        let dir =
            std::env::temp_dir().join(format!("ubilink-settings-{tag}-{}", std::process::id()));
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

    /// Parity (Tauri `fitWindow`): Settings' height is its content's, so every adapter
    /// row shows. 632 pt card (the 640 pt panel inside its 4 pt glass frame, D26), 20
    /// pt sides, 140 pt switches 8 pt apart: 4 per row.
    #[test]
    fn settings_height_fits_every_adapter_row() {
        // rc.6: the grid sits in the control column (632 - 2 x 20 - label 112 - gap 8 =
        // 472 pt): three 140 pt switches with 8 pt gaps a row.
        assert_eq!(adapter_columns(crate::shell::panel::CARD_WIDTH), 3);
        assert_eq!(adapter_columns(640.0), 3);
        assert_eq!(adapter_columns(100.0), 1, "never zero columns");
        let h = |adapters, confirming_reset, login_explained| {
            content_height(Content {
                adapters,
                confirming_reset,
                login_explained,
            })
        };
        // pad 12+12, title 24, four groups padded 8+8 (64) with 3 hairlines, then
        // Permissions 24 + its description 16, Data 24 (D26: macOS's 24 pt controls),
        // Login 20, App adapters: a gap of 4 and the hint 18 under the grid: 221 + grid.
        assert_eq!(h(0, false, false), 221.0);
        // 10 adapters (fixtures/adapters.json): 4 rows of 16 with 3 gaps of 8 = 88.
        assert_eq!(h(10, false, false), 309.0);
        // ceil(n / 3) rows: 3 is one row, 4 is two.
        assert_eq!(h(3, false, false), 221.0 + 16.0);
        assert_eq!(h(4, false, false), 221.0 + 16.0 + 8.0 + 16.0);
        assert_eq!(h(6, false, false), h(4, false, false));
        // The reset question adds its row and a gap; an explanation three lines.
        assert_eq!(h(10, true, false), 309.0 + 40.0 + 4.0);
        assert_eq!(h(10, false, true), 309.0 + 48.0);
        // Taller than the old fixed 140 pt once a second row of adapters exists.
        assert!(h(5, false, false) > 140.0);
    }

    /// rc.4: the Automation status Label, from the check onboarding uses. Granted is
    /// exactly `Allowed`; denied and never-asked are `Not allowed` (the Tauri pill's
    /// `automation === 'granted'`); a check that could not decide never claims either.
    #[test]
    fn automation_status_maps_the_permission_report() {
        let report = |automation: &str| -> Result<Report, String> {
            Ok(Report {
                automation: automation.into(),
            })
        };
        assert_eq!(automation_status_text(None), "Checking…");
        assert_eq!(automation_status_text(Some(&report("granted"))), "Allowed");
        assert_eq!(
            automation_status_text(Some(&report("denied"))),
            "Not allowed"
        );
        assert_eq!(
            automation_status_text(Some(&report("not_determined"))),
            "Not allowed"
        );
        assert_eq!(
            automation_status_text(Some(&Err("no permission fixture".into()))),
            "Not checked yet"
        );
    }

    /// Export as the Tauri app did: into Downloads when there is one, else home.
    #[test]
    fn export_goes_to_downloads_else_home() {
        let home = std::env::temp_dir().join(format!("ubilink-export-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(export_dir_in(&home), home, "no Downloads: home");
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        assert_eq!(export_dir_in(&home), home.join("Downloads"));
        std::fs::remove_dir_all(&home).ok();
    }

    /// No usable $HOME: refuse rather than write somewhere relative.
    #[test]
    fn export_refuses_a_missing_or_relative_home() {
        assert!(home_from(None).is_err());
        assert!(home_from(Some("".into())).is_err());
        assert!(home_from(Some("Users/x".into())).is_err());
        assert_eq!(
            home_from(Some("/Users/x".into())),
            Ok(std::path::PathBuf::from("/Users/x"))
        );
    }

    /// The reveal is `open -R <path>` as one argv (no shell), whatever the path holds.
    #[test]
    fn reveal_is_open_dash_r_with_the_path_as_one_argument() {
        let path = std::path::Path::new("/Users/x/Downloads/a b; rm -rf ~ $(x).json");
        let (program, args) = reveal_command(path);
        assert_eq!(program, "/usr/bin/open");
        assert_eq!(
            args,
            vec![
                std::ffi::OsString::from("-R"),
                path.as_os_str().to_os_string()
            ]
        );
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

    /// B1: Settings names the build as the Tauri panel did, `Ubilink <version>`, and
    /// that version is the app version in this crate's Cargo.toml [package].
    #[test]
    fn settings_version_label_names_the_app_version() {
        let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read desktop-gpui/Cargo.toml");
        let package = toml
            .split("\n[")
            .find(|section| section.starts_with("[package]") || section.starts_with("package]"))
            .expect("a [package] section");
        let version = package
            .lines()
            .find_map(|l| l.trim().strip_prefix("version = \""))
            .and_then(|v| v.strip_suffix('"'))
            .expect("a [package] version");
        assert_eq!(version_text(), format!("Ubilink {version}"));
        assert!(!version.is_empty());
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
