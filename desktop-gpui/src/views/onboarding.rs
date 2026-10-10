//! The onboarding view (Phase 1 R9): one step per missing macOS permission, each
//! with a button that opens that permission's System Settings pane, and a Re-check
//! (`R`, or the Re-check button) that re-reads the permission state in place — the
//! same window, the same steps, their statuses updated.
//!
//! What it renders (probes/README.md, "Onboarding (R9)"):
//! - one `ListItem` row per step (`Accessibility`, `Automation`), its subtree holding
//!   the permission's name and a status Label, `Not granted` or exactly `Granted`.
//!   Rows stay listed once granted, so the user sees the step they just finished turn
//!   green instead of watching it vanish. They are the only `ListItem`s in the view.
//! - in each row a `Button` whose text names the permission; Tab reaches it, Enter
//!   or Space (or a click) opens `ubilink_app::settings_pane_url(<pane>)` through the
//!   shell's [`Opener`] — the core's own launcher, recorded in the opener sink in
//!   automation builds so a probe never opens System Settings on the host (S2/O4).
//! - a hint line saying what to do next, or what failed and what to do (U3).
//!
//! Where the permission state comes from: the `permission` fixture when a probe has
//! loaded one (`crate::fixtures::path(Fixture::Permission)`, a JSON file shaped like
//! ubilink-app's `PermissionReport`); otherwise, in a release build, ubilink-app's
//! `permission_status()` (AXIsProcessTrusted + the osascript Automation probe), run
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

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::escape::{self, EscapeAction, EscapeState};
use super::theming::{self, FollowAppearance, Sample, Samples, ThemeSamples};
use super::Route;
use crate::opener::Opener;
use crate::theme::tokens::{font_size, radius, spacing};

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

const INTRO: &str = "Grant each permission below in System Settings, then press R to re-check.";
const ALL_SET: &str = "All set — press ⌘⇧K from any supported app.";
const STILL_WAITING: &str =
    "Still waiting — open the step's settings, grant it there, then press R to re-check.";
const OPENED: &str = "Opened System Settings — it may be behind this panel. Grant the permission there, then press R to re-check.";

/// The two macOS permissions ubilink asks for, in the order their steps are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Accessibility,
    Automation,
}

impl Permission {
    pub const ALL: [Permission; 2] = [Permission::Accessibility, Permission::Automation];

    pub fn name(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::Automation => "Automation",
        }
    }

    /// The pane name ubilink-app's `settings_pane_url` / `open_settings_with` take.
    pub fn pane(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::Automation => "automation",
        }
    }

    fn purpose(self) -> &'static str {
        match self {
            Permission::Accessibility => "Lets ubilink use the menus of apps like Bear.",
            Permission::Automation => "Lets ubilink read the link of the app you're in.",
        }
    }

    fn button_text(self) -> String {
        format!("Open {} settings", self.name())
    }
}

/// The permission state, shaped like ubilink-app's `PermissionReport` (and so like
/// the `permission` fixture files: `{"accessibility": bool, "automation": "granted" |
/// "denied" | "not_determined"}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Report {
    pub accessibility: bool,
    pub automation: String,
}

impl Report {
    pub fn granted(&self, p: Permission) -> bool {
        match p {
            Permission::Accessibility => self.accessibility,
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

/// Read the current permission state (see the module doc for where it comes from).
/// May block (the release osascript probe): call it off the main thread.
fn check_permissions() -> Result<Report, String> {
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
            accessibility: report.accessibility,
            automation: report.automation.into(),
        })
    }
}

/// Phase 2 R1: does a failed capture take the user here? Only when the permission
/// report says Automation is not granted — the one permission a capture needs (the
/// Tauri panel's `canUseCapture`, "Defect 5"). A missing Accessibility grant alone,
/// or no report at all (a failed check), leaves the failure on the capture view.
pub fn capture_failure_needs_onboarding(check: &Result<Report, String>) -> bool {
    matches!(check, Ok(report) if !report.granted(Permission::Automation))
}

/// Read the same permission report this view shows (the `permission` fixture, or in
/// a release build the live probe) and answer [`capture_failure_needs_onboarding`].
/// May block (the release osascript probe): call it off the main thread.
pub fn failed_capture_routes_here() -> bool {
    capture_failure_needs_onboarding(&check_permissions())
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
                            Button::new(id).small()
                        } else {
                            theming::accent_button(id)
                        }
                        .label(p.button_text())
                        .on_click(cx.listener(move |this, _, _, cx| this.open(p, cx)));
                        if sample {
                            d.child(SAMPLES.mark(Sample::Accent, button))
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // Escape dismisses (Tauri's generic handler, `views::escape`).
                if escape::is_escape(&event.keystroke) {
                    if escape::escape_action(Route::Onboarding, EscapeState::default())
                        == EscapeAction::Dismiss
                    {
                        escape::dismiss_panel(cx);
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
            // The opaque token surface (R14): the panel's blur stays behind the card.
            .bg(surface)
            .child(
                div()
                    .h(theming::ACCENT_H)
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .relative()
                            .text_size(font_size::TITLE)
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
                        Button::new("recheck")
                            .small()
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
    use super::{
        capture_failure_needs_onboarding, read_report, recheck_message, state, status_text,
        steps_after, Permission, Report, ALL_SET, GRANTED, NOT_GRANTED, UNKNOWN,
    };
    use Permission::{Accessibility, Automation};

    fn report(accessibility: bool, automation: &str) -> Result<Report, String> {
        Ok(Report {
            accessibility,
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
        let two_missing = report(false, "denied");
        let steps = steps_after(&[], &two_missing);
        assert_eq!(steps, [Accessibility, Automation]);
        assert_eq!(status_text(Automation, &two_missing), NOT_GRANTED);

        // Only Automation missing: one step, and it stays when granted.
        let one = steps_after(&[], &report(true, "not_determined"));
        assert_eq!(one, [Automation]);
        let granted = report(true, "granted");
        assert_eq!(steps_after(&one, &granted), [Automation]);
        assert_eq!(status_text(Automation, &granted), GRANTED);
        assert_eq!(recheck_message(&granted), ALL_SET);

        // Re-check after both were granted keeps both rows.
        assert_eq!(steps_after(&steps, &granted), [Accessibility, Automation]);

        // A failed check keeps the steps and says why.
        let failed: Result<Report, String> = Err("boom".into());
        assert_eq!(steps_after(&one, &failed), [Automation]);
        assert_eq!(status_text(Automation, &failed), UNKNOWN);
        assert_eq!(recheck_message(&failed), "boom");

        // Nothing missing on first look: show every permission as granted.
        assert_eq!(steps_after(&[], &granted), Permission::ALL);
    }

    #[test]
    fn failed_capture_routes_to_onboarding_only_when_automation_is_missing() {
        assert!(capture_failure_needs_onboarding(&report(false, "denied")));
        assert!(capture_failure_needs_onboarding(&report(
            true,
            "not_determined"
        )));
        // Automation granted: Accessibility alone never routes (Tauri "Defect 5").
        assert!(!capture_failure_needs_onboarding(&report(true, "granted")));
        assert!(!capture_failure_needs_onboarding(&report(false, "granted")));
        // No report: stay on the capture view with its failure.
        assert!(!capture_failure_needs_onboarding(&Err("no report".into())));
    }

    #[test]
    fn onboarding_pane_urls_are_ubilink_apps() {
        assert_eq!(
            ubilink_app::settings_pane_url(Automation.pane()),
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
        );
        assert!(ubilink_app::settings_pane_url(Accessibility.pane()).is_ok());
    }

    #[test]
    fn onboarding_reads_the_permission_fixture_shape() {
        let dir = std::env::temp_dir().join(format!("onboarding-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("p.json");
        std::fs::write(&ok, r#"{"accessibility": false, "automation": "denied"}"#).unwrap();
        assert_eq!(read_report(&ok), report(false, "denied"));
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "nope").unwrap();
        let err = read_report(&bad).unwrap_err();
        assert!(err.contains("press R to re-check"), "{err}");
        assert!(read_report(&dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
