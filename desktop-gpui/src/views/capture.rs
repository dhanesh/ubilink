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
//! Layout (D26 amendment, Things' Quick Entry): a leading link glyph, then the
//! captured title in the display size; the URL under it in secondary colour on the
//! title's left edge; the backlinks below in the browser's two-line rows with its grey
//! selection pill; and a footer bar (`theming::footer_bar`, on the grouped surface)
//! with the context bottom left (the panel's name, or `Copied`), and Cancel (does
//! what Escape does) and the one accent control, Copy link, as capsules bottom right.
//!
//! Every text a proof reads goes through GPUI's `text!` macro, so it reaches the
//! AccessKit tree as a `Label` (A11Y-NOTES.md): a bare `&str` child never does.
//!
//! `state()` is a context-free function (the router's `views::TABLE` row), so the
//! phase it reports lives in a module static the view writes: set to `capturing`
//! synchronously when the view is built (so the `trigger_hotkey` reply already reads
//! it), and settled only by the newest view's capture, so a stale result from an
//! earlier show can never overwrite a newer one.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use super::escape::{self, EscapeAction, EscapeState};
use super::layout::Layout;
use super::theming::{self, FollowAppearance, Sample, Samples, ThemeSamples};
use super::Route;
use crate::app_state;
use crate::theme::tokens::{font_size, radius, size, spacing, MONO_FONT_FAMILY};
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

// The layout (D26 amendment, Things-style Quick Entry), in points. The view sets
// these, so `content_height` adds up what it lays out, and the panel grows to show
// every backlink row (the Tauri panel's `fitWindow`) up to `MAX_ROWS`, past which the
// list scrolls.
/// The card's side inset: the body and the footer share it.
const INSET_X: Pixels = spacing::S4;
/// Above the title row.
const PAD_TOP: Pixels = spacing::S3;
/// Below the body, above the footer's hairline.
const PAD_BOTTOM: Pixels = spacing::S2;
/// The title row: the leading glyph, then the captured title (display size).
const TITLE_ROW_H: Pixels = size::CONTROL;
/// The leading glyph's box, and the gap after it: the title, the URL, the heading
/// and the backlinks' text all start this far in from [`INSET_X`].
const GLYPH: Pixels = spacing::S4;
const GLYPH_GAP: Pixels = spacing::S2;
/// The URL line under the title.
const URI_LINE_H: Pixels = px(16.);
/// Between the title block and the backlinks' heading.
const SECTION_GAP: Pixels = spacing::S2;
const HEADING_LINE_H: Pixels = px(16.);
/// One backlink row: the browser's two-line row (`theming::two_line_row`).
const ROW_H: Pixels = size::ROW_TWO_LINE;
/// Between the heading and the list (rows sit edge to edge, as in a macOS list).
const HEADING_GAP: Pixels = spacing::S1;
/// The most backlink rows the panel grows to show; more scroll.
pub const MAX_ROWS: usize = 8;

/// Where the title's text starts, from the card's left edge.
fn lead_x() -> Pixels {
    INSET_X + GLYPH + GLYPH_GAP
}

/// The capture panel's height with `backlinks` rows: none keeps the route's 140 pt;
/// otherwise the title block, the heading and the rows (at most [`MAX_ROWS`]), with
/// their gaps, and the footer, and never less than 140 pt.
pub fn content_height(backlinks: usize) -> f64 {
    let base = crate::shell::panel::PANEL_HEIGHT;
    if backlinks == 0 {
        return base;
    }
    let pt = |p: Pixels| f64::from(f32::from(p));
    let rows = backlinks.min(MAX_ROWS) as f64;
    let list = rows * pt(ROW_H);
    let title = pt(TITLE_ROW_H) + pt(URI_LINE_H);
    let section = pt(SECTION_GAP) + pt(HEADING_LINE_H) + pt(HEADING_GAP) + list;
    let h = pt(PAD_TOP) + title + section + pt(PAD_BOTTOM) + pt(theming::footer_h());
    h.max(base)
}

/// The cancel button's label: it does what Escape does (dismiss the panel).
pub const CANCEL_BUTTON: &str = "Cancel";

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
    /// Measured places (`views::layout`): `viewport`, the backlinks' scroll box
    /// (`backlinks_box`) and its last row (`last_backlink`), when there are backlinks.
    #[serde(default)]
    pub layout: BTreeMap<String, serde_json::Value>,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Capture(State {
        view: Route::Capture.as_str().into(),
        phase: lock().phase.into(),
        theme_samples: SAMPLES.get(),
        layout: LAYOUT.get(),
    })
}

/// The newest capture view's theme sample regions (R14), written at prepaint.
static SAMPLES: Samples = Samples::new();

/// The newest capture view's measured layout (`views::layout`), written at prepaint.
static LAYOUT: Layout = Layout::new();

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
            Err(e) => eprintln!("[ubilink] backlinks shown unordered: {e}"),
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
            eprintln!("[ubilink] backlinks shown unordered: unreadable export ({e})");
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
    /// The backlink count the panel was last fitted to; a change refits it.
    fitted: Option<usize>,
    /// The in-flight capture; dropped (cancelled) with the view when the panel hides.
    _capture: Task<()>,
}

impl CaptureView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Synchronous, before anything is awaited: the shell's reply to the hotkey
        // already reads `capturing` (render-first, TN1).
        let generation = lock().begin();
        let capture = cx.spawn(async move |this, cx| {
            // rc.4 (the Tauri panel's onboarding-until-Automation): while Automation is
            // not confirmed this session, a report saying it is not granted takes the
            // same panel to onboarding instead of capturing, so the capture's osascript
            // never runs (`onboarding::capture_open_needs_onboarding`). Once confirmed,
            // this answers at once without checking. The first frame already drew
            // `capturing` (render-first).
            let to_onboarding = cx
                .background_executor()
                .spawn(async { super::onboarding::opening_capture_routes_here() })
                .await;
            if to_onboarding {
                if lock().generation == generation {
                    cx.update(|cx| {
                        crate::shell::execute(crate::shell::Command::Open(Route::Onboarding), cx)
                    });
                }
                return;
            }
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
            fitted: None,
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

/// The backlinks section (R3): a heading, then one row per backlink in the order given
/// (newest link first), each row's name and URI as `text!` Labels (A11Y-NOTES.md), in
/// the browser's two-line row (`theming::two_line_row`), its text on the title's left
/// edge. The rows scroll past [`MAX_ROWS`]. They are the only `ListItem` nodes in the
/// capture view (probes/README.md). Each row tracks its own focus handle (`rows`, one
/// per backlink) under its own element id, so a Tab-focused row's `ListItem` node
/// itself is the accessibility tree's focused node (gpui-pre's div sets the a11y focus
/// for an id'd element whose tracked handle is focused). The focused row is the
/// selected one: the browser's grey pill (D26 amendment). Enter on it (the view's key
/// handler) or a click opens it (Phase 2 R4).
fn backlinks_section(
    backlinks: &[LinkView],
    rows: &[FocusHandle],
    window: &Window,
    cx: &mut Context<CaptureView>,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
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
                .flex_shrink_0()
                .h(ROW_H)
                .rounded(radius::ROW)
                .aria_position_in_set(ix + 1)
                .aria_size_of_set(count)
                .w_full()
                .min_w_0()
                .when(ix + 1 == count, |d| {
                    d.relative().child(LAYOUT.bounds("last_backlink"))
                })
                .child(
                    theming::two_line_row(
                        focused,
                        text!(
                            id = SharedString::from(format!("backlink-{ix}-name")),
                            link.name.clone()
                        ),
                        text!(
                            id = SharedString::from(format!("backlink-{ix}-uri")),
                            link.uri.clone()
                        ),
                        cx,
                    )
                    // Where the focused row's pill fill is, clear of its edge and
                    // (for a short title and URL) its text: `layout.selected_fill`,
                    // sampled by capture-things.
                    .when(focused, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(spacing::S2)
                                .right(spacing::S2)
                                .size(px(theming::SAMPLE_MIN))
                                .child(LAYOUT.bounds("selected_fill")),
                        )
                    }),
                )
        });
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .mt(SECTION_GAP)
        .gap(HEADING_GAP)
        .child(
            div()
                .pl(lead_x() - INSET_X)
                .text_size(font_size::META)
                .line_height(HEADING_LINE_H)
                .text_color(muted)
                .child(text!(BACKLINKS_HEADING)),
        )
        .child(
            // The scroll box, wrapped so its own place can be measured (a canvas
            // inside it would scroll with the rows). Its rows' pills start
            // `ROW_TEXT_INSET` left of the title, so their text lines up with it.
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .ml(lead_x() - INSET_X - theming::ROW_TEXT_INSET)
                .child(
                    div()
                        .id("backlinks")
                        .role(Role::List)
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(rows),
                )
                .child(LAYOUT.bounds("backlinks_box")),
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
        LAYOUT.clear();
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
        let shown = match &self.phase {
            Phase::Captured { backlinks, .. } => backlinks.len(),
            _ => 0,
        };
        if self.fitted != Some(shown) {
            // The capture landed with backlinks (or a new one replaced it): the panel
            // takes the rows' height, after this frame. Unchanged: nothing happens.
            self.fitted = Some(shown);
            let height = content_height(shown);
            cx.defer(move |cx| crate::shell::fit_panel(height, cx));
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
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        // The title row's text: what is being captured, the captured title (display
        // size, primary text), or the core's failure sentence (wrapping, U3).
        let title = match &self.phase {
            Phase::Capturing => div()
                .text_size(font_size::DISPLAY)
                .line_height(TITLE_ROW_H)
                .text_color(muted)
                .truncate()
                .child(text!(CAPTURING_TEXT))
                .into_any_element(),
            Phase::Captured { name, .. } => div()
                .relative()
                .text_size(font_size::DISPLAY)
                .line_height(TITLE_ROW_H)
                .text_color(foreground)
                .truncate()
                .child(text!(name.clone()))
                .child(SAMPLES.probe(Sample::Text))
                .into_any_element(),
            Phase::Failed { message } => div()
                .py(spacing::S1)
                .text_size(font_size::ROW)
                .text_color(danger)
                .child(text!(message.clone()))
                .into_any_element(),
        };
        let uri = match &self.phase {
            Phase::Captured { uri, .. } => Some(uri.clone()),
            _ => None,
        };
        // The footer's left side: the context (this panel, Things' "Inbox"), or the
        // copy's confirmation (R2).
        let (context_icon, context, context_color) = if copied {
            (IconName::Check, COPIED_TEXT, foreground)
        } else {
            (IconName::Inbox, Route::Capture.title(), muted)
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Escape dismisses (Tauri's generic handler, `views::escape`).
                if escape::is_escape(&event.keystroke) {
                    if escape::escape_action(Route::Capture, EscapeState::default())
                        == EscapeAction::Dismiss
                    {
                        escape::dismiss_panel(window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
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
            .rounded(radius::CARD)
            // The opaque card (R14, D26), inside the panel's glass frame.
            .bg(surface)
            // The body (Things' Quick Entry): the glyph and the title, the URL under
            // the title on the same left edge, then the backlinks.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px(INSET_X)
                    .pt(PAD_TOP)
                    .pb(PAD_BOTTOM)
                    .child(
                        div()
                            .flex_shrink_0()
                            .min_h(TITLE_ROW_H)
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(GLYPH_GAP)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .h(TITLE_ROW_H)
                                    .flex()
                                    .items_center()
                                    .child(
                                        Icon::default()
                                            .path(crate::theme::icons::LINK)
                                            .size(GLYPH)
                                            .text_color(muted),
                                    ),
                            )
                            .child(div().flex_1().min_w_0().child(title))
                            // An empty stretch of the card, for the theming probe.
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(theming::SAMPLE_MIN + 8.))
                                    .h(TITLE_ROW_H)
                                    .relative()
                                    .child(SAMPLES.probe(Sample::Surface)),
                            ),
                    )
                    .when_some(uri, |d, uri| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .pl(lead_x() - INSET_X)
                                .text_size(font_size::META)
                                .line_height(URI_LINE_H)
                                .font_family(MONO_FONT_FAMILY)
                                .text_color(muted)
                                .truncate()
                                .child(text!(uri)),
                        )
                    })
                    .when_some(backlinks, |d, section| d.child(section))
                    .when_some(self.open_error.clone(), |d, error| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .pl(lead_x() - INSET_X)
                                .text_size(font_size::META)
                                .text_color(danger)
                                .child(text!(error)),
                        )
                    })
                    // The graph couldn't be opened at start-up: captures work but
                    // aren't saved, and the user should know why (U3).
                    .when_some(app_state::startup_error(), |d, error| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .pl(lead_x() - INSET_X)
                                .text_size(font_size::META)
                                .text_color(muted)
                                .child(text!(error)),
                        )
                    }),
            )
            // The footer (Things' Quick Entry bar): a hairline, the grouped surface,
            // the context bottom left, Cancel and the one accent action, Copy link,
            // bottom right.
            .child(
                theming::footer_bar(INSET_X, theming::FooterSurface::Grouped, cx)
                    .relative()
                    .child(LAYOUT.bounds("footer"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(spacing::S1)
                            .h_full()
                            .relative()
                            // An empty stretch of the grouped surface at the context's
                            // right end: `layout.footer_fill`, sampled by capture-things.
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .size(px(theming::SAMPLE_MIN))
                                    .child(LAYOUT.bounds("footer_fill")),
                            )
                            .child(
                                Icon::new(context_icon)
                                    .size(font_size::META)
                                    .text_color(context_color),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(font_size::META)
                                    .text_color(context_color)
                                    .truncate()
                                    .child(text!(context)),
                            ),
                    )
                    // Does what Escape does: dismiss the panel. Not a tab stop:
                    // Escape is its keyboard path, and the view's Enter (copy) would
                    // otherwise fire on a focused Cancel; the Tab order stays rc.5's.
                    .child(
                        div()
                            .flex_shrink_0()
                            .relative()
                            .child(
                                theming::push_capsule_button("capture-cancel")
                                    .label(CANCEL_BUTTON)
                                    .tab_stop(false)
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        escape::dismiss_panel(window, cx);
                                    })),
                            )
                            .child(LAYOUT.bounds("cancel_button")),
                    )
                    // Always there (the footer never shifts when the capture lands);
                    // usable, and accent-filled, once there is a link to copy.
                    .child({
                        let button = theming::primary_capsule_button("capture-copy")
                            .label(COPY_BUTTON)
                            .disabled(!captured)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy(cx);
                            }));
                        div()
                            .flex_shrink_0()
                            .relative()
                            .map(|d| {
                                if captured {
                                    d.child(SAMPLES.mark(Sample::PrimaryCapsule, button))
                                } else {
                                    d.child(button)
                                }
                            })
                            .child(LAYOUT.bounds("copy_button"))
                    }),
            )
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
        content_height, is_copy_key, key_action, order_by_recency, state, KeyAction, Phase,
        Tracker, COPIED_HOLD, MAX_ROWS,
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

    /// Parity (Tauri `fitWindow`): the panel shows every backlink row, up to
    /// MAX_ROWS, past which the list scrolls; without backlinks it stays 140 pt.
    /// D26 amendment (Things-style): two-line rows and a footer bar.
    #[test]
    fn capture_height_fits_its_backlinks() {
        assert_eq!(content_height(0), 140.0);
        // top 12, title row 24, URL 16, gap 8, heading 16, gap 4, bottom 8, footer 41
        // (hairline 1, padding 8 + 8, control 24): 129 + rows of 40.
        assert_eq!(f32::from(crate::views::theming::footer_h()), 41.0);
        assert_eq!(content_height(1), 129.0 + 40.0);
        assert_eq!(content_height(3), 129.0 + 3.0 * 40.0);
        assert_eq!(content_height(4), 129.0 + 4.0 * 40.0);
        assert_eq!(content_height(MAX_ROWS + 5), content_height(MAX_ROWS));
        assert!(content_height(MAX_ROWS) <= crate::shell::panel::MAX_HEIGHT);
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
