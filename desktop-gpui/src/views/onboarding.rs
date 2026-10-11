//! The onboarding view (Phase 1 R9): one step per macOS permission capture needs,
//! each with a button that opens that permission's System Settings pane, and a
//! Re-check (`R`, or the Re-check button) that re-reads the permission state in
//! place — the same window, the same steps, their statuses updated.
//!
//! Automation is the only such permission. Accessibility once had a step for Bear's
//! menu scripting; Bear now captures through bearcli and no built-in adapter needs
//! Accessibility, so onboarding never asks for it (the Tauri panel dropped it too;
//! review S1 for rc.4). "All set" depends on Automation alone.
//!
//! What it renders (probes/README.md, "Onboarding (R9)"):
//! - one `ListItem` row per step (`Automation`), its subtree holding the permission's
//!   name and a status Label, `Not granted` or exactly `Granted`. A row stays listed
//!   once granted, so the user sees the step they just finished turn green instead of
//!   watching it vanish. They are the only `ListItem`s in the view.
//! - in each row a `Button` whose text names the permission; Tab reaches it, Enter
//!   or Space (or a click) opens `ubilink_app::settings_pane_url(<pane>)` through the
//!   shell's [`Opener`] — the core's own launcher, recorded in the opener sink in
//!   automation builds so a probe never opens System Settings on the host (S2/O4).
//! - a hint line saying what to do next, or what failed and what to do (U3).
//!
//! Where the permission state comes from: the `permission` fixture when a probe has
//! loaded one (`crate::fixtures::path(Fixture::Permission)`, a JSON file shaped like
//! ubilink-app's `PermissionReport`); otherwise, in a release build, ubilink-app's
//! `permission_status()` (its osascript Automation probe; the report's Accessibility
//! field is not read), run
//! off the main thread since osascript can take a while. An automation build never
//! runs that probe (it would raise a TCC prompt mid-probe, README "Shared state"):
//! without a fixture it says so instead.
//!
//! Keyboard: the automation `key` verb (and so this view) only sees key-down events.
//! GPUI's own Enter/Space-activates-a-focused-button path fires on key *up*, so each
//! step button sits in a wrapper whose `capture_key_down` handles Enter/Space while
//! the button inside it is focused, and calls `prevent_default` so the button's own
//! key-up click does not open the pane a second time for a real keyboard user.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::escape::{self, EscapeAction, EscapeState};
use super::theming::{self, FollowAppearance, Sample, Samples, ThemeSamples};
use super::Route;
use crate::opener::Opener;
use crate::theme::tokens::{font_size, font_weight, radius, spacing};

/// The onboarding route's view-state, for Task 6's automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
    /// Where the view's opaque surface, primary text and accent control are (R14,
    /// `views::theming`).
    #[serde(default)]
    pub theme_samples: ThemeSamples,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Onboarding(State {
        view: Route::Onboarding.as_str().into(),
        phase: "steps".into(),
        theme_samples: SAMPLES.get(),
    })
}

/// The newest onboarding view's theme sample regions (R14), written at prepaint.
static SAMPLES: Samples = Samples::new();

/// The status Labels (README contract: `Granted` is matched exactly, so `Not granted`
/// can never satisfy it).
const GRANTED: &str = "Granted";
const NOT_GRANTED: &str = "Not granted";
const UNKNOWN: &str = "Couldn't check";
const CHECKING: &str = "Checking permissions…";

const INTRO: &str = "Allow Automation for ubilink in System Settings, then press R to re-check.";
const ALL_SET: &str = "All set — press ⌘⇧K from any supported app.";
const STILL_WAITING: &str =
    "Still waiting on Automation — open its settings, allow ubilink there, then press R to re-check.";
const OPENED: &str = "Opened System Settings — it may be behind this panel. Grant the permission there, then press R to re-check.";

/// The macOS permissions ubilink asks for, in the order their steps are shown: only
/// Automation, the one capture needs (see the module doc for Accessibility).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Automation,
}

impl Permission {
    pub const ALL: [Permission; 1] = [Permission::Automation];

    pub fn name(self) -> &'static str {
        match self {
            Permission::Automation => "Automation",
        }
    }

    /// The pane name ubilink-app's `settings_pane_url` / `open_settings_with` take.
    pub fn pane(self) -> &'static str {
        match self {
            Permission::Automation => "automation",
        }
    }

    fn purpose(self) -> &'static str {
        match self {
            Permission::Automation => "Lets ubilink read the link of the app you're in.",
        }
    }

    fn button_text(self) -> String {
        format!("Open {} settings", self.name())
    }
}

/// The permission state, read from ubilink-app's `PermissionReport` (and so from the
/// `permission` fixture files: `{"accessibility": bool, "automation": "granted" |
/// "denied" | "not_determined"}`). Only Automation is kept: the `accessibility` key
/// is accepted and ignored, since nothing ubilink does needs it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Report {
    pub automation: String,
}

impl Report {
    pub fn granted(&self, p: Permission) -> bool {
        match p {
            Permission::Automation => self.automation == "granted",
        }
    }

    fn missing(&self) -> Vec<Permission> {
        Permission::ALL
            .into_iter()
            .filter(|p| !self.granted(*p))
            .collect()
    }
}

/// Which permissions get a step after a check, given the steps already shown.
/// The first check shows one step per missing permission (every permission when none
/// is missing, so the screen still says where things stand); later checks never drop
/// a step — a granted one stays listed as `Granted` — and add any that went missing.
/// A failed check changes nothing.
pub fn steps_after(shown: &[Permission], check: &Result<Report, String>) -> Vec<Permission> {
    let Ok(report) = check else {
        return shown.to_vec();
    };
    let missing = report.missing();
    if shown.is_empty() && missing.is_empty() {
        return Permission::ALL.to_vec();
    }
    Permission::ALL
        .into_iter()
        .filter(|p| shown.contains(p) || missing.contains(p))
        .collect()
}

/// The status Label for `p`'s step after `check`.
pub fn status_text(p: Permission, check: &Result<Report, String>) -> &'static str {
    match check {
        Ok(report) if report.granted(p) => GRANTED,
        Ok(_) => NOT_GRANTED,
        Err(_) => UNKNOWN,
    }
}

/// The line under the steps after a Re-check: a result, not just the standing hint,
/// so the key press visibly does something even when no status changed (Tauri's
/// "Defect 2"); a failed check says what failed and what to do (U3).
pub fn recheck_message(check: &Result<Report, String>) -> String {
    match check {
        Ok(report) if report.missing().is_empty() => ALL_SET.into(),
        Ok(_) => STILL_WAITING.into(),
        Err(e) => e.clone(),
    }
}

/// Parse a permission fixture file.
pub fn read_report(path: &Path) -> Result<Report, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "Couldn't read the permission state from {}: {e}. Fix the file, then press R to re-check.",
            path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|e| {
        format!(
            "The permission state in {} is not valid: {e}. Fix the file, then press R to re-check.",
            path.display()
        )
    })
}

/// Set once a permission check, or the start-up probe, says Automation is granted.
/// From then on, opening capture never checks again in this session, so onboarding
/// cannot come back for a user who is set up ([`capture_open_needs_onboarding`]).
static AUTOMATION_CONFIRMED: AtomicBool = AtomicBool::new(false);

/// Remember a check that says Automation is granted (never forgets one).
fn note_check(confirmed: &AtomicBool, check: &Result<Report, String>) {
    if matches!(check, Ok(report) if report.granted(Permission::Automation)) {
        confirmed.store(true, Ordering::Relaxed);
    }
}

/// Read the current permission state (see the module doc for where it comes from):
/// the one check onboarding, Settings' Permissions row and the capture paths share.
/// A check that finds Automation granted confirms the session and opens the core's
/// capture gate, which the start-up probe set: a grant made after launch works at
/// once, without a relaunch. May block (the release osascript probe): call it off
/// the main thread.
pub(crate) fn check_permissions() -> Result<Report, String> {
    let check = read_permissions();
    note_session(&check);
    check
}

/// [`note_check`] for this session, and pass a grant on to the core's capture gate.
fn note_session(check: &Result<Report, String>) {
    note_check(&AUTOMATION_CONFIRMED, check);
    open_capture_gate();
}

/// Once the session is confirmed, open the core's capture gate (only ever upgrades).
fn open_capture_gate() {
    if AUTOMATION_CONFIRMED.load(Ordering::Relaxed) {
        if let Ok(mut backend) = crate::app_state::get().backend.lock() {
            backend.note_automation_granted();
        }
    }
}

fn read_permissions() -> Result<Report, String> {
    if let Some(path) = crate::fixtures::path(crate::fixtures::Fixture::Permission) {
        return read_report(&path);
    }
    #[cfg(feature = "automation")]
    {
        Err("No permission fixture is loaded, so permissions were not checked. Load one, then press R to re-check.".into())
    }
    #[cfg(not(feature = "automation"))]
    {
        let report = ubilink_app::permission_status();
        Ok(Report {
            automation: report.automation.into(),
        })
    }
}

/// Phase 2 R1: does a failed capture take the user here? Only when the permission
/// report says Automation is not granted — the one permission a capture needs (the
/// Tauri panel's `canUseCapture`, "Defect 5"). No report at all (a failed check)
/// leaves the failure on the capture view.
pub fn capture_failure_needs_onboarding(check: &Result<Report, String>) -> bool {
    matches!(check, Ok(report) if !report.granted(Permission::Automation))
}

/// Read the same permission report this view shows (the `permission` fixture, or in
/// a release build the live probe) and answer [`capture_failure_needs_onboarding`].
/// May block (the release osascript probe): call it off the main thread.
pub fn failed_capture_routes_here() -> bool {
    capture_failure_needs_onboarding(&check_permissions())
}

/// rc.4: does opening capture (the hotkey, the tray's Capture, a reopen) take the
/// user here instead of capturing? The Tauri panel showed onboarding until Automation
/// was granted (`refreshPermissions`); here the rule is checked when capture opens,
/// so nothing pops up at launch:
/// - Automation already confirmed this session (`confirmed`): never; `check` is not
///   even run, so a set-up user pays nothing per capture.
/// - Otherwise run `check`: only a report that says Automation is not granted routes
///   here ([`capture_failure_needs_onboarding`]'s rule). A report that says granted
///   sets `confirmed`; a failed check (no report) lets the capture run.
pub fn capture_open_needs_onboarding(
    confirmed: &AtomicBool,
    check: impl FnOnce() -> Result<Report, String>,
) -> bool {
    if confirmed.load(Ordering::Relaxed) {
        return false;
    }
    let check = check();
    note_check(confirmed, &check);
    capture_failure_needs_onboarding(&check)
}

/// [`capture_open_needs_onboarding`] on this session's state: confirmed when the
/// core's capture gate is open (a release build's start-up probe said granted) or
/// any earlier check said granted, else the same report this view shows. May block
/// (the release osascript probe, only until Automation is confirmed): call it off
/// the main thread.
pub fn opening_capture_routes_here() -> bool {
    let gate_open = crate::app_state::get()
        .backend
        .lock()
        .is_ok_and(|b| b.automation_permission() == ubilink_core::PermissionState::Granted);
    if gate_open {
        AUTOMATION_CONFIRMED.store(true, Ordering::Relaxed);
    }
    let routes = capture_open_needs_onboarding(&AUTOMATION_CONFIRMED, read_permissions);
    open_capture_gate();
    routes
}

/// Open `p`'s System Settings pane through the core's launcher, wrapped in the
/// shell's opener (the opener sink in automation builds). Returns the hint to show.
fn open_pane(p: Permission) -> String {
    match ubilink_app::open_settings_with(&Opener(ubilink_app::SpawnRunner), p.pane()) {
        Ok(()) => OPENED.into(),
        Err(e) => format!(
            "Couldn't open System Settings ({e}). Open it yourself: Privacy & Security → {}.",
            p.name()
        ),
    }
}

/// Enter or Space with no modifiers: the keys that activate a focused button.
fn is_activation_key(keystroke: &Keystroke) -> bool {
    (keystroke.key == "enter" || keystroke.key == "space") && !keystroke.modifiers.modified()
}

/// `R` with no modifiers: Re-check.
fn is_recheck_key(keystroke: &Keystroke) -> bool {
    keystroke.key == "r" && !keystroke.modifiers.modified()
}

pub struct OnboardingView {
    /// `None` until the first check answers.
    check: Option<Result<Report, String>>,
    steps: Vec<Permission>,
    /// What to do next, or the last action's result.
    hint: Option<String>,
    /// Held so the view handles keys from its first frame: GPUI dispatches keys from
    /// the focused element up, and with nothing focused neither Tab nor `R` arrive.
    focus: FocusHandle,
    focused: bool,
    /// Repaints in the system's light or dark tokens while open (R14).
    appearance: FollowAppearance,
    /// The in-flight check; dropped (cancelled) with the view.
    _check: Task<()>,
}

impl OnboardingView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut view = OnboardingView {
            check: None,
            steps: Vec::new(),
            hint: None,
            focus: cx.focus_handle(),
            focused: false,
            appearance: FollowAppearance::default(),
            _check: Task::ready(()),
        };
        view.start_check(false, cx);
        view
    }

    /// Re-read the permission state off the main thread, then update the steps in
    /// place. `recheck` makes the hint report the result (vs. the standing intro).
    fn start_check(&mut self, recheck: bool, cx: &mut Context<Self>) {
        if recheck {
            self.hint = Some(CHECKING.into());
            cx.notify();
        }
        self._check = cx.spawn(async move |this, cx| {
            let check = cx
                .background_executor()
                .spawn(async { check_permissions() })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.apply(check, recheck);
                cx.notify();
            });
        });
    }

    fn apply(&mut self, check: Result<Report, String>, recheck: bool) {
        self.steps = steps_after(&self.steps, &check);
        self.hint = Some(match (&check, recheck) {
            (_, true) | (Err(_), false) => recheck_message(&check),
            (Ok(report), false) if report.missing().is_empty() => ALL_SET.into(),
            (Ok(_), false) => INTRO.into(),
        });
        self.check = Some(check);
    }

    /// Whether the latest check says `p` is granted (`false` before it answers).
    fn granted(&self, p: Permission) -> bool {
        self.check
            .as_ref()
            .is_some_and(|check| status_text(p, check) == GRANTED)
    }

    fn open(&mut self, p: Permission, cx: &mut Context<Self>) {
        self.hint = Some(open_pane(p));
        cx.notify();
    }

    /// One step. `sample` marks its button as the view's accent sample (R14): the
    /// first step still to grant, whose button is the accent-filled one.
    fn step_row(
        &self,
        ix: usize,
        p: Permission,
        sample: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let granted = self.granted(p);
        let status = match &self.check {
            Some(check) => status_text(p, check),
            None => UNKNOWN,
        };
        let foreground = theming::primary_text(cx);
        let theme = cx.theme();
        let status_color = if granted { theme.success } else { theme.danger };
        div()
            .id(SharedString::from(format!("step-{}", p.pane())))
            .role(Role::ListItem)
            .aria_position_in_set(ix + 1)
            .aria_size_of_set(self.steps.len())
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::S3)
            .w_full()
            .min_w_0()
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(96.))
                    .text_size(font_size::ROW)
                    .text_color(foreground)
                    .child(text!(
                        id = SharedString::from(format!("step-{}-name", p.pane())),
                        p.name()
                    )),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(96.))
                    .text_size(font_size::META)
                    .text_color(status_color)
                    .child(text!(
                        id = SharedString::from(format!("step-{}-status", p.pane())),
                        status
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(font_size::META)
                    .text_color(theme.muted_foreground)
                    .child(text!(
                        id = SharedString::from(format!("step-{}-purpose", p.pane())),
                        p.purpose()
                    )),
            )
            .child(
                // Enter/Space reach this wrapper only while the button inside it is
                // focused (keys go from the focused element up); see the module doc.
                div()
                    .flex_shrink_0()
                    .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if is_activation_key(&event.keystroke) {
                            this.open(p, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    .map(|d| {
                        let id = SharedString::from(format!("open-{}", p.pane()));
                        // Still to grant: the accent (primary) button, the step's call
                        // to action. Granted: a plain one, to look again.
                        let button = if granted {
                            theming::push_button(id)
                        } else {
                            theming::primary_button(id)
                        }
                        .label(p.button_text())
                        .on_click(cx.listener(move |this, _, _, cx| this.open(p, cx)));
                        if sample {
                            d.child(SAMPLES.mark(Sample::Primary, button))
                        } else {
                            d.child(button)
                        }
                    }),
            )
            .into_any_element()
    }
}

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        SAMPLES.clear();
        self.appearance.ensure(window, cx);
        let steps = self.steps.clone();
        let first_missing = steps.iter().position(|p| !self.granted(*p));
        let rows: Vec<AnyElement> = steps
            .iter()
            .enumerate()
            .map(|(ix, p)| self.step_row(ix, *p, first_missing == Some(ix), cx))
            .collect();
        let hint = self.hint.clone().unwrap_or_else(|| CHECKING.to_string());
        let failed = matches!(self.check, Some(Err(_)));
        let surface = theming::surface(cx);
        let foreground = theming::primary_text(cx);
        let theme = cx.theme();
        div()
            .track_focus(&self.focus)
            // Bubble phase: `R` from anywhere in the view, including a focused step
            // button (a Button binds nothing to it).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Escape dismisses (Tauri's generic handler, `views::escape`).
                if escape::is_escape(&event.keystroke) {
                    if escape::escape_action(Route::Onboarding, EscapeState::default())
                        == EscapeAction::Dismiss
                    {
                        escape::dismiss_panel(window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                if is_recheck_key(&event.keystroke) {
                    this.start_check(true, cx);
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
            // The opaque card (R14, D26), inside the panel's glass frame.
            .bg(surface)
            .child(
                div()
                    .h(theming::CONTROL_H)
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .relative()
                            .text_size(font_size::TITLE)
                            .font_weight(font_weight::HEADLINE)
                            .text_color(foreground)
                            .child(text!(Route::Onboarding.title()))
                            .child(SAMPLES.probe(Sample::Text)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .relative()
                            .child(SAMPLES.probe(Sample::Surface)),
                    ),
            )
            .child(
                div()
                    .id("onboarding-steps")
                    .role(Role::List)
                    .flex()
                    .flex_col()
                    .gap(spacing::S1)
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::S3)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(font_size::META)
                            .text_color(if failed {
                                theme.danger
                            } else {
                                theme.muted_foreground
                            })
                            .child(text!(id = "onboarding-hint", hint)),
                    )
                    .child(
                        theming::push_button("recheck")
                            .label("Re-check")
                            .on_click(cx.listener(|this, _, _, cx| this.start_check(true, cx))),
                    ),
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        capture_failure_needs_onboarding, capture_open_needs_onboarding, read_report,
        recheck_message, state, status_text, steps_after, Permission, Report, ALL_SET, GRANTED,
        NOT_GRANTED, STILL_WAITING, UNKNOWN,
    };
    use Permission::Automation;

    fn report(automation: &str) -> Result<Report, String> {
        Ok(Report {
            automation: automation.into(),
        })
    }

    #[test]
    fn state_names_its_own_route() {
        let json = state().to_json();
        assert_eq!(json["view"], "onboarding");
        assert_eq!(json["phase"], "steps");
    }

    #[test]
    fn onboarding_steps_one_per_missing_permission_and_kept_once_granted() {
        // Automation is the only step, whether it is denied or never asked.
        let denied = report("denied");
        let steps = steps_after(&[], &denied);
        assert_eq!(steps, [Automation]);
        assert_eq!(status_text(Automation, &denied), NOT_GRANTED);
        assert_eq!(recheck_message(&denied), STILL_WAITING);
        assert_eq!(steps_after(&[], &report("not_determined")), [Automation]);

        // It stays listed once granted, and setup is complete.
        let granted = report("granted");
        assert_eq!(steps_after(&steps, &granted), [Automation]);
        assert_eq!(status_text(Automation, &granted), GRANTED);
        assert_eq!(recheck_message(&granted), ALL_SET);

        // A failed check keeps the steps and says why.
        let failed: Result<Report, String> = Err("boom".into());
        assert_eq!(steps_after(&steps, &failed), [Automation]);
        assert_eq!(status_text(Automation, &failed), UNKNOWN);
        assert_eq!(recheck_message(&failed), "boom");

        // Nothing missing on first look: show the step as granted.
        assert_eq!(steps_after(&[], &granted), Permission::ALL);
    }

    /// Review S1 (rc.4): Automation granted with Accessibility not is a finished setup.
    /// The fixture's `accessibility: false` is read and ignored: no step asks for it,
    /// and Re-check says All set rather than waiting forever.
    #[test]
    fn automation_alone_completes_setup_whatever_accessibility_says() {
        let dir = std::env::temp_dir().join(format!("onboarding-s1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ax-missing.json");
        std::fs::write(
            &path,
            r#"{"accessibility": false, "automation": "granted"}"#,
        )
        .unwrap();
        let check = read_report(&path);
        assert_eq!(check, report("granted"));
        assert_eq!(steps_after(&[], &check), [Automation]);
        assert_eq!(status_text(Automation, &check), GRANTED);
        assert_eq!(recheck_message(&check), ALL_SET);
        assert!(!capture_failure_needs_onboarding(&check));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_capture_routes_to_onboarding_only_when_automation_is_missing() {
        assert!(capture_failure_needs_onboarding(&report("denied")));
        assert!(capture_failure_needs_onboarding(&report("not_determined")));
        assert!(!capture_failure_needs_onboarding(&report("granted")));
        // No report: stay on the capture view with its failure.
        assert!(!capture_failure_needs_onboarding(&Err("no report".into())));
    }

    #[test]
    fn opening_capture_routes_to_onboarding_until_automation_is_confirmed() {
        let confirmed = AtomicBool::new(false);
        // Not granted (denied, or never asked): onboarding, and nothing remembered.
        assert!(capture_open_needs_onboarding(&confirmed, || report(
            "denied"
        )));
        assert!(capture_open_needs_onboarding(&confirmed, || report(
            "not_determined"
        )));
        assert!(!confirmed.load(Ordering::Relaxed));
        // No report: capture runs (its own failure route still applies).
        assert!(!capture_open_needs_onboarding(&confirmed, || Err(
            "no report".into()
        )));
        assert!(!capture_open_needs_onboarding(&confirmed, || report(
            "granted"
        )));
        // That check said granted: from now on the check is not even run, and a
        // stale not-granted report cannot bring onboarding back.
        assert!(confirmed.load(Ordering::Relaxed));
        assert!(!capture_open_needs_onboarding(&confirmed, || {
            panic!("a confirmed session must not check again")
        }));
    }

    #[test]
    fn onboarding_pane_urls_are_ubilink_apps() {
        assert_eq!(
            ubilink_app::settings_pane_url(Automation.pane()),
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        );
    }

    #[test]
    fn onboarding_reads_the_permission_fixture_shape() {
        let dir = std::env::temp_dir().join(format!("onboarding-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("p.json");
        std::fs::write(&ok, r#"{"accessibility": false, "automation": "denied"}"#).unwrap();
        assert_eq!(read_report(&ok), report("denied"));
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "nope").unwrap();
        let err = read_report(&bad).unwrap_err();
        assert!(err.contains("press R to re-check"), "{err}");
        assert!(read_report(&dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
