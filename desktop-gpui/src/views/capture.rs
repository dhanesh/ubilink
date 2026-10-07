//! The capture view (Phase 1 R1; spec TN1 "render-first, fill-async").
//!
//! The view opens in the `capturing` phase with no capture data — the panel's first
//! frame never waits on the core — and fills in when the core answers: the captured
//! resource's name and URI (`captured`), or the core's sentence saying what failed and
//! what to do (`failed`, U3). The slow part (an `osascript` round trip to the frontmost
//! app, or a fixture's delay) runs on GPUI's background executor, in a task the view
//! owns, so hiding the panel drops it.
//!
//! Where the capture comes from:
//! - **release build:** `ubilink_app::capture_now` on the one shared state
//!   (`app_state::get()`), exactly the Tauri shell's capture.
//! - **automation build:** the `capture` fixture (`crate::fixtures`, spec O4), fed to
//!   the core through a fixture [`ResourceAdapter`] so it takes the same
//!   `PanelBackend::capture_current` path a live capture does: validated by the core's
//!   one policy (D16, S2), stored in the graph, backlinks read. With no fixture loaded
//!   an automation build never runs a live capture (no TCC prompt or focus theft in a
//!   probe); it shows a failure that says so.
//!
//! Every text a proof reads goes through GPUI's `text!` macro, so it reaches the
//! AccessKit tree as a `Label` (A11Y-NOTES.md): a bare `&str` child never does.
//!
//! `state()` is a context-free function (the router's `views::TABLE` row), so the
//! phase it reports lives in a module static the view writes: set to `capturing`
//! synchronously when the view is built (so the `trigger_hotkey` reply already reads
//! it), and settled only by the newest view's capture, so a stale result from an
//! earlier show can never overwrite a newer one.

use std::sync::Mutex;

use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::Route;
use crate::app_state;
use crate::theme::tokens::{font_size, radius, spacing, MONO_FONT_FAMILY};

/// The capturing indicator's text (the Tauri panel's wording, `dist/panel.js`).
pub const CAPTURING_TEXT: &str = "Capturing the active resource…";

/// The copied confirmation's text (R2): shown once the URI is on the pasteboard.
pub const COPIED_TEXT: &str = "Copied";

/// Is `keystroke` one of the copy keys (R2, U2)? Plain Enter, or ⌘C (Ctrl-C too, as
/// the Tauri panel's handler accepts, `dist/panel.js`). Shift or Option make it some
/// other shortcut, so they don't count.
pub fn is_copy_key(keystroke: &Keystroke) -> bool {
    let m = &keystroke.modifiers;
    if m.alt || m.shift || m.function {
        return false;
    }
    match keystroke.key.as_str() {
        "enter" => !m.platform && !m.control,
        "c" => m.platform != m.control,
        _ => false,
    }
}

/// The capture route's view-state, for the automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    /// `"capturing"` from the moment the view is built until the core answers, then
    /// `"captured"` or `"failed"` (probes/README.md, "Phase 1 view-state contract").
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Capture(State {
        view: Route::Capture.as_str().into(),
        phase: lock().phase.into(),
    })
}

/// Where the capture stands. The view renders from this alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Shown, waiting for the core. No capture data yet.
    Capturing,
    /// The core captured and stored the frontmost resource.
    Captured { name: String, uri: String },
    /// The capture failed; `message` is the core's sentence, shown verbatim (U3).
    Failed { message: String },
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Capturing => "capturing",
            Phase::Captured { .. } => "captured",
            Phase::Failed { .. } => "failed",
        }
    }

    /// What a copy puts on the pasteboard: the captured URI, and nothing before the
    /// capture lands or after it fails (the Tauri panel's `canConfirm`).
    pub fn copy_payload(&self) -> Option<&str> {
        match self {
            Phase::Captured { uri, .. } => Some(uri),
            _ => None,
        }
    }

    /// The core's answer, as the phase it leads to.
    pub fn from_core(result: Result<ubilink_core::PanelView, String>) -> Phase {
        match result {
            Ok(view) => Phase::Captured {
                name: view.resource.name,
                uri: view.resource.uri,
            },
            Err(message) => Phase::Failed { message },
        }
    }
}

/// Which capture the reported phase belongs to: every new view starts a new
/// generation, and only that generation may settle the phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tracker {
    generation: u64,
    phase: &'static str,
}

impl Tracker {
    const fn new() -> Self {
        Tracker {
            generation: 0,
            phase: "capturing",
        }
    }

    /// A view was built: a new capture starts, in `capturing`.
    fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.phase = "capturing";
        self.generation
    }

    /// Capture `generation` finished. Ignored (returns false) when a newer view has
    /// started since.
    fn settle(&mut self, generation: u64, phase: &Phase) -> bool {
        if generation != self.generation {
            return false;
        }
        self.phase = phase.as_str();
        true
    }
}

static CURRENT: Mutex<Tracker> = Mutex::new(Tracker::new());

fn lock() -> std::sync::MutexGuard<'static, Tracker> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The automation build's capture source (and the headless test's): a capture fixture
/// fed through the core. Absent from a release build, which captures live.
#[cfg(any(feature = "automation", test))]
mod fixture {
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::Deserialize;
    use ubilink_core::{CaptureFailure, OpenPlan, PanelBackend, ResourceAdapter, ResourceIdentity};

    use super::Phase;

    /// Feeds a fixture's result to the core's capture path (`PanelBackend::capture_current`)
    /// in place of a live app adapter, so a fixture capture is validated and stored exactly
    /// as a live one is. Needs no permission: it reads no app.
    struct FixtureAdapter(Result<ResourceIdentity, String>);

    impl ResourceAdapter for FixtureAdapter {
        fn id(&self) -> &str {
            "fixture"
        }

        fn needs_permission(&self) -> bool {
            false
        }

        fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
            self.0
                .clone()
                .map_err(|explanation| CaptureFailure::AdapterError { explanation })
        }

        fn open_plan(&self, resource: &ResourceIdentity) -> OpenPlan {
            ubilink_core::open_plan_for(ubilink_core::Platform::MacOS, &resource.uri)
        }
    }

    /// Capture through the core with `outcome` standing in for the frontmost app.
    pub(super) fn capture_through_core(
        backend: &Mutex<PanelBackend>,
        outcome: Result<ResourceIdentity, String>,
        now: i64,
    ) -> Phase {
        let adapter = FixtureAdapter(outcome);
        Phase::from_core(PanelBackend::capture_current(backend, &adapter, now))
    }

    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    pub(super) fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// A capture fixture file (probes/fixtures/README.md, "Capture fixtures"): either a
    /// success (`name`, `uri`, `app`, optional `delay_ms`) or a failure (`error` only).
    #[derive(Debug, Deserialize)]
    struct FixtureFile {
        name: Option<String>,
        uri: Option<String>,
        delay_ms: Option<u64>,
        error: Option<String>,
    }

    /// A parsed capture fixture: how long to wait, then what the "frontmost app" returns.
    #[derive(Debug, Clone, PartialEq)]
    pub(super) struct FixtureCapture {
        pub(super) delay: std::time::Duration,
        pub(super) outcome: Result<ResourceIdentity, String>,
    }

    pub(super) fn parse_fixture(json: &str) -> Result<FixtureCapture, String> {
        let file: FixtureFile = serde_json::from_str(json).map_err(|e| {
            format!("The capture fixture isn't valid JSON ({e}); fix the file and capture again.")
        })?;
        let delay = std::time::Duration::from_millis(file.delay_ms.unwrap_or(0));
        let outcome = match (file.error, file.name, file.uri) {
            (Some(error), _, _) => Err(error),
            (None, Some(name), Some(uri)) => Ok(ResourceIdentity::new(name, uri)),
            _ => {
                return Err(
                    "The capture fixture has neither a name and uri nor an error; fix the file and capture again."
                        .into(),
                )
            }
        };
        Ok(FixtureCapture { delay, outcome })
    }
}

/// Run one capture off the main thread and return the phase it ends in.
async fn run_capture(cx: &mut AsyncApp) -> Phase {
    let executor = cx.background_executor().clone();
    #[cfg(feature = "automation")]
    {
        let Some(path) = crate::fixtures::path(crate::fixtures::Fixture::Capture) else {
            return Phase::Failed {
                message: "No capture fixture is loaded, so nothing was captured; load a capture fixture, then capture again."
                    .into(),
            };
        };
        let parsed = executor
            .spawn(async move {
                std::fs::read_to_string(&path)
                    .map_err(|e| {
                        format!(
                            "Couldn't read the capture fixture {} ({e}); load a readable one and capture again.",
                            path.display()
                        )
                    })
                    .and_then(|json| fixture::parse_fixture(&json))
            })
            .await;
        let capture = match parsed {
            Ok(f) => f,
            Err(message) => return Phase::Failed { message },
        };
        if !capture.delay.is_zero() {
            executor.timer(capture.delay).await;
        }
        executor
            .spawn(async move {
                fixture::capture_through_core(
                    &app_state::get().backend,
                    capture.outcome,
                    fixture::now_ms(),
                )
            })
            .await
    }
    #[cfg(not(feature = "automation"))]
    {
        executor
            .spawn(async { Phase::from_core(ubilink_app::capture_now(app_state::get())) })
            .await
    }
}

pub struct CaptureView {
    phase: Phase,
    /// Holds keyboard focus from the first frame, so Enter and ⌘C reach this view's
    /// key handler: GPUI dispatches keys from the focused element up its path.
    focus: FocusHandle,
    focused: bool,
    /// The URI is on the pasteboard: the `Copied` confirmation shows. The panel stays
    /// open after a copy, so the confirmation stays until the panel next hides.
    copied: bool,
    /// The in-flight capture; dropped (cancelled) with the view when the panel hides.
    _capture: Task<()>,
}

impl CaptureView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Synchronous, before anything is awaited: the shell's reply to the hotkey
        // already reads `capturing` (render-first, TN1).
        let generation = lock().begin();
        let capture = cx.spawn(async move |this, cx| {
            let phase = run_capture(cx).await;
            let _ = this.update(cx, |view, cx| {
                if lock().settle(generation, &phase) {
                    view.phase = phase;
                    cx.notify();
                }
            });
        });
        CaptureView {
            phase: Phase::Capturing,
            focus: cx.focus_handle(),
            focused: false,
            copied: false,
            _capture: capture,
        }
    }

    /// Enter or ⌘C: put the captured URI on the macOS pasteboard (GPUI's platform
    /// clipboard, i.e. `NSPasteboard.generalPasteboard`) and confirm. Returns whether
    /// there was anything to copy.
    fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(uri) = self.phase.copy_payload() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(uri.to_string()));
        self.copied = true;
        cx.notify();
        true
    }
}

impl Render for CaptureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        let copied = self.copied;
        let theme = cx.theme();
        let body = match &self.phase {
            Phase::Capturing => div()
                .text_size(font_size::TITLE)
                .text_color(theme.muted_foreground)
                .child(text!(CAPTURING_TEXT))
                .into_any_element(),
            Phase::Captured { name, uri } => div()
                .flex()
                .flex_col()
                .gap(spacing::S1)
                .child(
                    div()
                        .text_size(font_size::DISPLAY)
                        .text_color(theme.foreground)
                        .truncate()
                        .child(text!(name.clone())),
                )
                .child(
                    div()
                        .text_size(font_size::META)
                        .font_family(MONO_FONT_FAMILY)
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(text!(uri.clone())),
                )
                .into_any_element(),
            Phase::Failed { message } => div()
                .text_size(font_size::ROW)
                .text_color(theme.danger)
                .child(text!(message.clone()))
                .into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if is_copy_key(&event.keystroke) && this.copy(cx) {
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(spacing::S2)
            .p(spacing::S5)
            .rounded(radius::CARD)
            .child(
                div()
                    .text_size(font_size::META)
                    .text_color(theme.muted_foreground)
                    .child(text!(Route::Capture.title())),
            )
            .child(body)
            .when(copied, |d| {
                d.child(
                    div()
                        .text_size(font_size::META)
                        .text_color(theme.foreground)
                        .child(text!(COPIED_TEXT)),
                )
            })
            // The graph couldn't be opened at start-up: captures work but aren't
            // saved, and the user should know why (U3).
            .when_some(app_state::startup_error(), |d, error| {
                d.child(
                    div()
                        .text_size(font_size::META)
                        .text_color(theme.muted_foreground)
                        .child(text!(error)),
                )
            })
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
    use super::fixture::{capture_through_core, parse_fixture};
    use super::{is_copy_key, state, Phase, Tracker};
    use std::sync::Mutex;
    use ubilink_core::{
        AdapterRegistry, LinkGraph, PanelBackend, ResourceIdentity, UbilinkService,
    };

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("probes/fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The shared state's backend, as `app_state::build` makes it, on an in-memory graph.
    fn backend() -> Mutex<PanelBackend> {
        let schemes = AdapterRegistry::defaults().declared_schemes();
        Mutex::new(PanelBackend::new(
            UbilinkService::new(LinkGraph::open_in_memory().unwrap()).allow_schemes(schemes),
        ))
    }

    /// R1's view model, headless: capturing → captured with the fixture's name and URI
    /// through the core; capturing → failed with the fixture's sentence verbatim; a
    /// stale capture never settles a newer view's phase.
    #[test]
    fn capture_view_states() {
        // The state verb names the route and one of the three phases.
        let json = state().to_json();
        assert_eq!(json["view"], "capture");
        assert!(
            ["capturing", "captured", "failed"].contains(&json["phase"].as_str().unwrap()),
            "{json}"
        );

        // A new view starts capturing (render-first).
        let mut t = Tracker::new();
        let first = t.begin();
        assert_eq!(t.phase, "capturing");

        // The delayed fixture: 1 s delay, then a capture the core validates and stores.
        let delayed = parse_fixture(&fixture("capture-delayed.json")).unwrap();
        assert_eq!(delayed.delay, std::time::Duration::from_millis(1000));
        let b = backend();
        let captured = capture_through_core(&b, delayed.outcome, 1_000);
        assert_eq!(
            captured,
            Phase::Captured {
                name: "Delayed Capture Target".into(),
                uri: "https://example.com/delayed".into(),
            }
        );
        assert!(t.settle(first, &captured));
        assert_eq!(t.phase, "captured");
        let stored = b.lock().unwrap().list_links().unwrap();
        assert!(
            stored
                .iter()
                .any(|e| e.uri == "https://example.com/delayed"),
            "the capture is stored in the graph"
        );

        // The ok fixture resolves with no delay.
        let ok = parse_fixture(&fixture("capture-ok.json")).unwrap();
        assert!(ok.delay.is_zero());
        assert!(matches!(
            capture_through_core(&b, ok.outcome, 2_000),
            Phase::Captured { .. }
        ));

        // The failure fixture: the sentence reaches the view verbatim, no stale data.
        let second = t.begin();
        assert_eq!(t.phase, "capturing", "a reopen starts capturing again");
        let failed_fixture = parse_fixture(&fixture("capture-failed.json")).unwrap();
        let failed = capture_through_core(&b, failed_fixture.outcome, 3_000);
        assert_eq!(
            failed,
            Phase::Failed {
                message: "Safari returned no active resource — open something there and try again."
                    .into()
            }
        );
        assert!(t.settle(second, &failed));
        assert_eq!(t.phase, "failed");

        // A capture from an earlier show finishing late is ignored.
        let third = t.begin();
        assert!(
            !t.settle(second, &captured),
            "stale generation must not settle"
        );
        assert_eq!(t.phase, "capturing");
        assert!(t.settle(third, &captured));

        // The core's validation policy still applies to fixture captures (D16, S2).
        let rejected = capture_through_core(
            &b,
            Ok(ResourceIdentity::new("Bad", "javascript:alert(1)")),
            4_000,
        );
        match rejected {
            Phase::Failed { message } => assert!(!message.trim().is_empty()),
            other => panic!("a denied scheme must fail, got {other:?}"),
        }

        // Malformed fixtures fail with a message, not a panic.
        assert!(parse_fixture("{}").is_err());
        assert!(parse_fixture("not json").is_err());
    }

    /// R2's key model, headless: Enter and ⌘C (and Ctrl-C, as Tauri) copy; other keys
    /// and modified Enters don't; only a captured phase has anything to copy.
    #[test]
    fn copy_keys_and_payload() {
        let key = |k: &str| gpui_kit::Keystroke::parse(k).unwrap();
        for k in ["enter", "cmd-c", "ctrl-c"] {
            assert!(is_copy_key(&key(k)), "{k} copies");
        }
        for k in [
            "c",
            "cmd-enter",
            "shift-enter",
            "cmd-shift-c",
            "alt-c",
            "cmd-ctrl-c",
            "escape",
            "cmd-v",
        ] {
            assert!(!is_copy_key(&key(k)), "{k} must not copy");
        }

        let captured = Phase::Captured {
            name: "Example".into(),
            uri: "https://example.com/".into(),
        };
        assert_eq!(captured.copy_payload(), Some("https://example.com/"));
        assert_eq!(Phase::Capturing.copy_payload(), None);
        assert_eq!(
            Phase::Failed {
                message: "x".into()
            }
            .copy_payload(),
            None
        );
    }
}
