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

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::component::{ActiveTheme, Disableable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::theming::{self, FollowAppearance, Sample, Samples, ThemeSamples};
use super::Route;
use crate::app_state;
use crate::theme::tokens::{font_size, radius, spacing, MONO_FONT_FAMILY};
use ubilink_core::{LinkView, PanelBackend, PanelView};

/// The capturing indicator's text (the Tauri panel's wording, `dist/panel.js`).
pub const CAPTURING_TEXT: &str = "Capturing the active resource…";

/// The copied confirmation's text (R2): shown once the URI is on the pasteboard.
pub const COPIED_TEXT: &str = "Copied";

/// How long the `Copied` confirmation stays up before the panel hides itself (Phase 2
/// R2; the Tauri panel's `confirmCopy` then `dismiss_panel`). At least 1 s, so the user
/// sees it (probes/README.md, "Phase 1 view-state contract"), and well inside the 3 s
/// from the key by which the panel must be gone.
pub const COPIED_HOLD: Duration = Duration::from_millis(1300);

/// The backlinks section's heading (R3; the Tauri panel's wording, `dist/index.html`).
pub const BACKLINKS_HEADING: &str = "Linked to this";

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

/// What a key does in the capture view (Phase 2 R4, spec TN2: focus decides).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Put the captured URI on the pasteboard (R2).
    Copy,
    /// Open backlink row `n` (0-based, display order) through the core opener.
    Open(usize),
}

/// The action `keystroke` takes when backlink row `focused_row` (if any) holds keyboard
/// focus: plain Enter on a focused row opens that row and does not copy; every other
/// copy key (Enter with no row focused, ⌘C anywhere) copies; anything else does nothing.
pub fn key_action(keystroke: &Keystroke, focused_row: Option<usize>) -> Option<KeyAction> {
    if !is_copy_key(keystroke) {
        return None;
    }
    match (keystroke.key.as_str(), focused_row) {
        ("enter", Some(row)) => Some(KeyAction::Open(row)),
        _ => Some(KeyAction::Copy),
    }
}

/// The capture route's view-state, for the automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    /// `"capturing"` from the moment the view is built until the core answers, then
    /// `"captured"` or `"failed"` (probes/README.md, "Phase 1 view-state contract").
    pub phase: String,
    /// Where the view's opaque surface, primary text and accent control are (R14,
    /// `views::theming`).
    #[serde(default)]
    pub theme_samples: ThemeSamples,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Capture(State {
        view: Route::Capture.as_str().into(),
        phase: lock().phase.into(),
        theme_samples: SAMPLES.get(),
    })
}

/// The newest capture view's theme sample regions (R14), written at prepaint.
static SAMPLES: Samples = Samples::new();

/// The copy button's label (R2's click path; Enter and ⌘C are the keyboard path).
pub const COPY_BUTTON: &str = "Copy link";

/// Where the capture stands. The view renders from this alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Shown, waiting for the core. No capture data yet.
    Capturing,
    /// The core captured and stored the frontmost resource. `backlinks` are the
    /// resources linked to it (R3), newest link first.
    Captured {
        name: String,
        uri: String,
        backlinks: Vec<LinkView>,
    },
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

    /// The core's answer, as the phase it leads to. The backlinks keep the order
    /// they come in; [`settle_capture`] puts them in recency order first.
    pub fn from_core(result: Result<PanelView, String>) -> Phase {
        match result {
            Ok(view) => Phase::Captured {
                name: view.resource.name,
                uri: view.resource.uri,
                backlinks: view.backlinks,
            },
            Err(message) => Phase::Failed { message },
        }
    }
}

/// The core's capture answer, with its backlinks put newest link first (R3), as the
/// phase it leads to. Runs on the background executor, after the capture has
/// released the backend lock.
///
/// Which resources are backlinks is the core's answer (`PanelView::backlinks`), so the
/// graph's own query decides membership. The core lists them by URI and exposes no
/// link timestamps, so the order comes from the graph's export (`export_graph`, the
/// one public read that carries each link's `created_at`). If that read fails the
/// backlinks still show, in the core's order: a missing order is not worth hiding them.
fn settle_capture(backend: &Mutex<PanelBackend>, result: Result<PanelView, String>) -> Phase {
    let Ok(mut view) = result else {
        return Phase::from_core(result);
    };
    if view.backlinks.len() > 1 {
        let export = backend
            .lock()
            .map_err(|_| "internal state was poisoned".to_string())
            .and_then(|b| b.export_graph());
        match export {
            Ok(json) => order_by_recency(&view.resource.uri, &mut view.backlinks, &json),
            Err(e) => eprintln!("[ubilink-next] backlinks shown unordered: {e}"),
        }
    }
    Phase::from_core(Ok(view))
}

/// Sort `backlinks` of `subject` newest link first, by the link `created_at` in
/// `export` (the graph's `ubilink.graph.v3` export JSON). A link with no timestamp,
/// or a backlink the export doesn't know, sorts last; ties keep their order.
fn order_by_recency(subject: &str, backlinks: &mut [LinkView], export: &str) {
    #[derive(Deserialize)]
    struct Export {
        resources: Vec<ExportResource>,
        links: Vec<ExportLink>,
    }
    #[derive(Deserialize)]
    struct ExportResource {
        guid: String,
        uri: String,
    }
    #[derive(Deserialize)]
    struct ExportLink {
        a: String,
        b: String,
        created_at: Option<i64>,
    }
    let export: Export = match serde_json::from_str(export) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[ubilink-next] backlinks shown unordered: unreadable export ({e})");
            return;
        }
    };
    let uri_of: HashMap<&str, &str> = export
        .resources
        .iter()
        .map(|r| (r.guid.as_str(), r.uri.as_str()))
        .collect();
    let mut linked_at: HashMap<&str, i64> = HashMap::new();
    for link in &export.links {
        let (Some(&a), Some(&b)) = (uri_of.get(link.a.as_str()), uri_of.get(link.b.as_str()))
        else {
            continue;
        };
        let other = if a == subject {
            b
        } else if b == subject {
            a
        } else {
            continue;
        };
        let at = link.created_at.unwrap_or(i64::MIN);
        let slot = linked_at.entry(other).or_insert(i64::MIN);
        *slot = (*slot).max(at);
    }
    backlinks.sort_by_key(|l| {
        std::cmp::Reverse(linked_at.get(l.uri.as_str()).copied().unwrap_or(i64::MIN))
    });
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

    /// Capture through the core with `outcome` standing in for the frontmost app,
    /// then settle it as a live capture is (backlinks newest first).
    pub(super) fn capture_through_core(
        backend: &Mutex<PanelBackend>,
        outcome: Result<ResourceIdentity, String>,
        now: i64,
    ) -> Phase {
        let adapter = FixtureAdapter(outcome);
        super::settle_capture(
            backend,
            PanelBackend::capture_current(backend, &adapter, now),
        )
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
            .spawn(async {
                let state = app_state::get();
                settle_capture(&state.backend, ubilink_app::capture_now(state))
            })
            .await
    }
}

pub struct CaptureView {
    phase: Phase,
    /// Holds keyboard focus from the first frame, so Enter and ⌘C reach this view's
    /// key handler: GPUI dispatches keys from the focused element up its path.
    focus: FocusHandle,
    focused: bool,
    /// The URI is on the pasteboard: the `Copied` confirmation shows, and stays until
    /// the panel hides itself `COPIED_HOLD` later (Phase 2 R2).
    copied: bool,
    /// One focus handle per backlink row, in display order: each row is a tab stop, so
    /// Tab (gpui-base `Root`'s `focus_next`) steps through them one at a time, and the
    /// focused row is what Enter opens (Phase 2 R4).
    rows: Vec<FocusHandle>,
    /// A failed backlink open: the core's reason and what to do (U3).
    open_error: Option<String>,
    /// The pending self-dismiss after a copy; dropped (cancelled) with the view, so a
    /// panel that already hid or switched route is never hidden by a stale copy.
    _dismiss: Option<Task<()>>,
    /// Repaints in the system's light or dark tokens while open (R14).
    appearance: FollowAppearance,
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
            let failed = matches!(phase, Phase::Failed { .. });
            let settled = this
                .update(cx, |view, cx| {
                    let settled = lock().settle(generation, &phase);
                    if settled {
                        view.phase = phase;
                        cx.notify();
                    }
                    settled
                })
                .unwrap_or(false);
            if !(settled && failed) {
                return;
            }
            // Phase 2 R1 (the Tauri panel's capture-err handler): the failure shows
            // first, then the permission report decides. Automation not granted takes
            // the user to where it is fixed (U3): the same panel window switches to
            // onboarding. Granted, or no report, keeps the failure here.
            let to_onboarding = cx
                .background_executor()
                .spawn(async { super::onboarding::failed_capture_routes_here() })
                .await;
            // Hiding or re-routing the panel drops this view and cancels this task, so
            // reaching here means the panel is still open on this capture; a newer
            // capture (generation) owns the route instead.
            if to_onboarding && lock().generation == generation {
                cx.update(|cx| {
                    crate::shell::execute(crate::shell::Command::Open(Route::Onboarding), cx)
                });
            }
        });
        CaptureView {
            phase: Phase::Capturing,
            focus: cx.focus_handle(),
            focused: false,
            rows: Vec::new(),
            open_error: None,
            copied: false,
            _dismiss: None,
            appearance: FollowAppearance::default(),
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
        // Phase 2 R2 (U2: the copy ends the interaction): keep `Copied` up for
        // `COPIED_HOLD`, then hide the panel as a dismiss does. A second copy restarts
        // the hold. The task is the view's, so it dies with the view.
        self._dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_HOLD).await;
            if this.upgrade().is_some() {
                cx.update(|cx| crate::shell::execute(crate::shell::Command::Hide, cx));
            }
        }));
        true
    }

    /// The backlink row holding keyboard focus, if any.
    fn focused_row(&self, window: &Window) -> Option<usize> {
        self.rows.iter().position(|h| h.is_focused(window))
    }

    /// Open backlink row `ix` (Enter on the focused row, or a click; the Tauri panel's
    /// backlink anchors) through ubilink-app's `open_link_with`, so the core re-validates
    /// the URI (D16) and the shell's `Opener` seam decides what runs: the real launcher
    /// in a release build, the `opened` sink in an automation build (S2). Nothing is
    /// copied, and the panel stays open, as in Tauri. A failed open says what to do (U3).
    fn open_backlink(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Phase::Captured { backlinks, .. } = &self.phase else {
            return;
        };
        let Some(uri) = backlinks.get(ix).map(|l| l.uri.clone()) else {
            return;
        };
        let opened = ubilink_app::open_link_with(
            app_state::get(),
            &crate::opener::Opener(ubilink_app::SpawnRunner),
            &uri,
        );
        self.open_error = opened
            .err()
            .map(|e| super::browser::open_failed_text(&uri, &e));
        cx.notify();
    }
}

/// The backlinks section (R3): a heading, then one `ListItem` row per backlink in
/// the order given (newest link first), each row's name and URI as `text!` Labels
/// (A11Y-NOTES.md). The panel is short, so the rows are one line each and scroll.
/// They are the only `ListItem` nodes in the capture view (probes/README.md). Each row
/// tracks its own focus handle (`rows`, one per backlink) under its own element id, so
/// a Tab-focused row's `ListItem` node itself is the accessibility tree's focused node
/// (gpui-pre's div sets the a11y focus for an id'd element whose tracked handle is
/// focused). Enter on it (the view's key handler) or a click opens it (Phase 2 R4).
fn backlinks_section(
    backlinks: &[LinkView],
    rows: &[FocusHandle],
    window: &Window,
    cx: &mut Context<CaptureView>,
) -> AnyElement {
    let (foreground, muted) = (theming::primary_text(cx), cx.theme().muted_foreground);
    let focus_bg = cx.theme().list_active;
    let count = backlinks.len();
    let rows = backlinks
        .iter()
        .zip(rows)
        .enumerate()
        .map(|(ix, (link, handle))| {
            let focused = handle.is_focused(window);
            div()
                .id(SharedString::from(format!("backlink-{ix}")))
                .role(Role::ListItem)
                .track_focus(handle)
                .on_click(cx.listener(move |this, _, _, cx| this.open_backlink(ix, cx)))
                .cursor_pointer()
                .rounded(radius::CARD)
                .when(focused, |d| d.bg(focus_bg))
                .aria_position_in_set(ix + 1)
                .aria_size_of_set(count)
                .flex()
                .flex_row()
                .items_baseline()
                .line_height(px(18.))
                .gap(spacing::S3)
                .w_full()
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(relative(0.45))
                        .text_size(font_size::ROW)
                        .text_color(foreground)
                        .truncate()
                        .child(text!(
                            id = SharedString::from(format!("backlink-{ix}-name")),
                            link.name.clone()
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(font_size::META)
                        .font_family(MONO_FONT_FAMILY)
                        .text_color(muted)
                        .truncate()
                        .child(text!(
                            id = SharedString::from(format!("backlink-{ix}-uri")),
                            link.uri.clone()
                        )),
                )
        });
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap(px(2.))
        .child(
            div()
                .text_size(font_size::META)
                .line_height(px(16.))
                .text_color(muted)
                .child(text!(BACKLINKS_HEADING)),
        )
        .child(
            div()
                .id("backlinks")
                .role(Role::List)
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

impl Render for CaptureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus, cx);
        }
        SAMPLES.clear();
        self.appearance.ensure(window, cx);
        let copied = self.copied;
        let captured = matches!(self.phase, Phase::Captured { .. });
        let row_count = match &self.phase {
            Phase::Captured { backlinks, .. } => backlinks.len(),
            _ => 0,
        };
        if self.rows.len() != row_count {
            self.rows = (0..row_count)
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect();
        }
        let backlinks = match &self.phase {
            Phase::Captured { backlinks, .. } if !backlinks.is_empty() => {
                Some(backlinks_section(backlinks, &self.rows, window, cx))
            }
            _ => None,
        };
        let surface = theming::surface(cx);
        let foreground = theming::primary_text(cx);
        let theme = cx.theme();
        let body = match &self.phase {
            Phase::Capturing => div()
                .text_size(font_size::TITLE)
                .text_color(theme.muted_foreground)
                .child(text!(CAPTURING_TEXT))
                .into_any_element(),
            // With backlinks below, name and URI share one line so the rows fit
            // the short panel.
            Phase::Captured { name, uri, .. } if backlinks.is_some() => div()
                .flex()
                .flex_row()
                .items_baseline()
                .line_height(px(20.))
                .gap(spacing::S3)
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(relative(0.5))
                        .relative()
                        .text_size(font_size::TITLE)
                        .text_color(foreground)
                        .truncate()
                        .child(text!(name.clone()))
                        .child(SAMPLES.probe(Sample::Text)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(font_size::META)
                        .font_family(MONO_FONT_FAMILY)
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(text!(uri.clone())),
                )
                .into_any_element(),
            Phase::Captured { name, uri, .. } => div()
                .flex()
                .flex_col()
                .gap(spacing::S1)
                .child(
                    div()
                        .relative()
                        .text_size(font_size::DISPLAY)
                        .text_color(foreground)
                        .truncate()
                        .child(text!(name.clone()))
                        .child(SAMPLES.probe(Sample::Text)),
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match key_action(&event.keystroke, this.focused_row(window)) {
                    Some(KeyAction::Open(row)) => {
                        this.open_backlink(row, cx);
                        cx.stop_propagation();
                    }
                    Some(KeyAction::Copy) if this.copy(cx) => cx.stop_propagation(),
                    Some(KeyAction::Copy) | None => {}
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .when(backlinks.is_none(), |d| {
                d.justify_center().gap(spacing::S2).p(spacing::S5)
            })
            .when(backlinks.is_some(), |d| {
                d.gap(spacing::S1).px(spacing::S5).py(spacing::S2)
            })
            .rounded(radius::CARD)
            // The opaque token surface (R14): the panel's blur stays behind the card.
            .bg(surface)
            // Header: the title, then the copy button.
            // Fixed height, so the content doesn't jump when the capture lands.
            .child(
                div()
                    .h(theming::ACCENT_H)
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::S3)
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(font_size::META)
                            .text_color(theme.muted_foreground)
                            .child(text!(Route::Capture.title())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .relative()
                            .child(SAMPLES.probe(Sample::Surface)),
                    )
                    // Always there (the header never shifts when the capture lands);
                    // usable, and accent-filled, once there is a link to copy.
                    .map(|d| {
                        let button = theming::accent_button("capture-copy")
                            .label(COPY_BUTTON)
                            .disabled(!captured)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy(cx);
                            }));
                        if captured {
                            d.child(SAMPLES.mark(Sample::Accent, button))
                        } else {
                            d.child(button)
                        }
                    }),
            )
            .child(body)
            .when_some(backlinks, |d, section| d.child(section))
            .when_some(self.open_error.clone(), |d, error| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(font_size::META)
                        .text_color(theme.danger)
                        .child(text!(error)),
                )
            })
            .when(copied, |d| {
                d.child(
                    div()
                        .text_size(font_size::META)
                        .text_color(foreground)
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
    use super::{
        is_copy_key, key_action, order_by_recency, state, KeyAction, Phase, Tracker, COPIED_HOLD,
    };
    use std::sync::Mutex;
    use std::time::Duration;
    use ubilink_core::{
        AdapterRegistry, LinkGraph, LinkView, PanelBackend, ResourceIdentity, UbilinkService,
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
                backlinks: vec![],
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

    /// Phase 2 R2: the confirmation is held at least 1 s (the frozen view contract)
    /// and the panel is gone well inside 3 s of the key.
    #[test]
    fn copied_hold_meets_the_contract() {
        assert!(COPIED_HOLD >= Duration::from_secs(1));
        assert!(COPIED_HOLD <= Duration::from_millis(1500));
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
            backlinks: vec![],
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

    /// Phase 2 R4's key model, headless (spec TN2): Enter on a focused backlink row
    /// opens that row and does not copy; Enter with no row focused copies; ⌘C copies
    /// whether or not a row is focused; other keys do nothing.
    #[test]
    fn enter_opens_the_focused_row_else_copies() {
        let key = |k: &str| gpui_kit::Keystroke::parse(k).unwrap();
        assert_eq!(key_action(&key("enter"), None), Some(KeyAction::Copy));
        assert_eq!(key_action(&key("enter"), Some(1)), Some(KeyAction::Open(1)));
        assert_eq!(key_action(&key("enter"), Some(0)), Some(KeyAction::Open(0)));
        assert_eq!(key_action(&key("cmd-c"), Some(1)), Some(KeyAction::Copy));
        assert_eq!(key_action(&key("cmd-c"), None), Some(KeyAction::Copy));
        for k in ["shift-enter", "cmd-enter", "space", "tab", "escape"] {
            assert_eq!(key_action(&key(k), Some(1)), None, "{k}");
        }
    }

    /// The backlinks of a captured `phase`, as (name, uri) pairs in display order.
    fn listed(phase: &Phase) -> Vec<(String, String)> {
        match phase {
            Phase::Captured { backlinks, .. } => backlinks
                .iter()
                .map(|l| (l.name.clone(), l.uri.clone()))
                .collect(),
            other => panic!("expected a capture, got {other:?}"),
        }
    }

    /// R3's view model, headless, on the probe's own fixtures: the subject's 3
    /// backlinks newest link first and never the decoy; after the newer fixture is
    /// merged, a re-capture (the view reopening) lists the fourth one first.
    #[test]
    fn backlinks_newest_first() {
        let b = backend();
        b.lock()
            .unwrap()
            .import_graph(&fixture("graph-backlinks.json"))
            .unwrap();
        let subject = || {
            parse_fixture(&fixture("capture-subject.json"))
                .unwrap()
                .outcome
        };
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(n, u)| (n.to_string(), u.to_string()))
                .collect()
        };

        let first = capture_through_core(&b, subject(), 1_800_000_000_000);
        assert_eq!(
            listed(&first),
            pairs(&[
                ("Gamma Doc", "https://gamma.example/three"),
                ("Beta Post", "https://beta.example/two"),
                ("Alpha Paper", "https://alpha.example/one"),
            ])
        );

        b.lock()
            .unwrap()
            .import_graph(&fixture("graph-backlinks-newer.json"))
            .unwrap();
        let reopened = capture_through_core(&b, subject(), 1_800_000_001_000);
        let listed = listed(&reopened);
        assert_eq!(
            listed,
            pairs(&[
                ("Delta Memo", "https://delta.example/four"),
                ("Gamma Doc", "https://gamma.example/three"),
                ("Beta Post", "https://beta.example/two"),
                ("Alpha Paper", "https://alpha.example/one"),
            ])
        );
        assert!(listed
            .iter()
            .all(|(_, u)| u != "https://decoy.example/unrelated"));
    }

    /// The ordering itself: links from either endpoint count, a missing timestamp
    /// sorts last, unknown rows keep their place among equals, and an unreadable
    /// export leaves the core's order alone.
    #[test]
    fn order_by_recency_rules() {
        let link = |u: &str| LinkView {
            name: u.into(),
            uri: u.into(),
        };
        let export = r#"{"schema":"ubilink.graph.v3",
            "resources":[{"guid":"s","uri":"s:"},{"guid":"x","uri":"x:"},{"guid":"y","uri":"y:"},{"guid":"z","uri":"z:"}],
            "links":[{"a":"x","b":"s","created_at":5},{"a":"s","b":"y","created_at":9},{"a":"s","b":"z","created_at":null},{"a":"x","b":"y","created_at":99}]}"#;
        let mut v = vec![link("z:"), link("x:"), link("w:"), link("y:")];
        order_by_recency("s:", &mut v, export);
        let uris: Vec<&str> = v.iter().map(|l| l.uri.as_str()).collect();
        assert_eq!(uris, ["y:", "x:", "z:", "w:"]);

        let mut v = vec![link("b:"), link("a:")];
        order_by_recency("s:", &mut v, "not json");
        assert_eq!(v, [link("b:"), link("a:")]);
    }
}
