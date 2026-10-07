//! The link browser (Phase 1 R4: a virtualised list of every captured link).
//!
//! Rows come from ubilink-app's `list_links` on the one shared state
//! (`app_state::get()`), in its order (`updated_at` DESC, then rowid DESC: the Tauri
//! browser's, B1). They render through gpui-component's `List`, which lays them out
//! with `v_virtual_list`: only the rows that cross the viewport (plus one overdraw
//! row) are built, so 5,000 links cost the same few row elements as 5 do. `List` wraps
//! each row in a `ListItem` node carrying `selected` and `size_of_set`; a row's name
//! and URI are `text!` Labels inside it (A11Y-NOTES.md), on one line, because the
//! panel is 140 pt tall and at least five rows must show.
//!
//! Render-first, fill-async (TN1): the view opens with the list in its loading state
//! and fills when the graph query, run on the background executor, answers. A query
//! that fails shows the core's message and what to do (U3).
//!
//! The filter `Input` holds keyboard focus from the first frame (probes/README.md,
//! "Phase 1 view-state contract"). A focused single-line Input swallows Home and End,
//! so this view takes them in the capture phase on its root and stops propagation
//! (the a11y spike's proven pattern, A11Y-NOTES.md): End selects and shows the last
//! row, Home the first.
//!
//! The filter (R5) ranks by the normative rule in `probes/fixtures/README.md`
//! (`scripts/probe.py`'s `rank()`): a link matches when the typed text is an in-order
//! subsequence of its name or its URI (case-folded), non-matches are dropped, and
//! matches sort by the shortest window of either field that embeds the query, then
//! that window's start, then the URI. Every keystroke that changes the filter text
//! re-filters synchronously and is timed into `filter_samples` / `filter_p99_ms`
//! (the view-state contract), reset each time the view is built.
//!
//! Automation builds record a `view_first_frame` metric (route `browser`): from the
//! view's construction to just after its first frame's present, the same
//! next-frame-then-defer timing `shell/panel.rs` uses for the hotkey.

use std::sync::Mutex;
use std::time::Instant;

use gpui_kit::component::input::{Input, InputEvent as FilterEvent, InputState, MoveEnd, MoveHome};
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme, IndexPath, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use ubilink_core::BrowseEntry;

use super::Route;
use crate::app_state;
use crate::theme::tokens::{font_size, radius, spacing, MONO_FONT_FAMILY};

/// The filter's placeholder (the Tauri browser's wording, `dist/index.html`).
pub const FILTER_PLACEHOLDER: &str = "Filter links…";

/// Shown in place of rows when the graph has none.
pub const EMPTY_TEXT: &str = "No links yet. Capture one with the capture hotkey.";

// Layout budget for the 140 pt panel (`shell::panel::PANEL_SIZE`). The browser test
// checks it leaves room for at least five rows (the probe's first-rows check) and,
// with the virtual list's one overdraw row, never builds more than sixty.
/// Top and bottom padding of the view.
const PAD_Y: Pixels = spacing::S2;
/// The header row: the route title and the filter.
const HEADER_H: Pixels = px(28.);
/// Between the header and the list.
const GAP: Pixels = spacing::S1;
/// One row: name and URI on a single line.
const ROW_H: Pixels = px(20.);
/// Up to this many shown rows, the list is laid out at its full height inside a
/// scrolling container, so every row is built and reaches the accessibility tree (a
/// filtered result is read whole); past it, the list virtualises to its viewport.
/// R4's bound: never more than sixty row elements.
const ALL_ROWS_MAX: usize = 60;

/// Shown in place of rows when links exist but none matches the filter.
pub const NO_MATCH_TEXT: &str = "No links match the filter. Delete a character or two.";

/// The browser route's view-state, for the automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
    /// Re-filters timed since the view was last built (one per text change).
    pub filter_samples: usize,
    /// Nearest-rank p99 of those re-filter times, in ms; `null` before the first.
    pub filter_p99_ms: Option<f64>,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    let timings = timings();
    super::ViewState::Browser(State {
        view: Route::Browser.as_str().into(),
        phase: "browsing".into(),
        filter_samples: timings.len(),
        filter_p99_ms: p99(&timings),
    })
}

/// The newest view's re-filter times in ms. `state()` has no view to ask, so they
/// live here: cleared when a view is built, appended by that view's filter.
static FILTER_MS: Mutex<Vec<f64>> = Mutex::new(Vec::new());

fn filter_ms() -> std::sync::MutexGuard<'static, Vec<f64>> {
    FILTER_MS.lock().unwrap_or_else(|e| e.into_inner())
}

fn timings() -> Vec<f64> {
    filter_ms().clone()
}

/// Nearest-rank p99 (the smallest sample with at least 99 % of samples at or below
/// it), as `metrics.summary` computes it: no interpolation.
fn p99(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (0.99 * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// The shortest window of `text` that embeds `query` as an in-order subsequence, as
/// `(length, start)` in chars, preferring the earliest start among equally short
/// windows; `None` when `query` is empty or not a subsequence of `text`. A port of
/// `scripts/probe.py`'s `min_window_span` (minimum window subsequence, not the
/// greedy-leftmost embedding): after each forward match ending at `end`, walk back to
/// the latest start that still embeds the query, then resume just after that start.
pub fn min_window_span(query: &[char], text: &[char]) -> Option<(usize, usize)> {
    let (n, m) = (text.len(), query.len());
    if m == 0 {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < n {
        let (mut j, mut k) = (0, i);
        while k < n {
            if text[k] == query[j] {
                j += 1;
                if j == m {
                    break;
                }
            }
            k += 1;
        }
        if j != m {
            break;
        }
        let end = k;
        let mut start = end;
        let mut left = m;
        loop {
            if text[start] == query[left - 1] {
                left -= 1;
                if left == 0 {
                    break;
                }
            }
            start -= 1;
        }
        let candidate = (end - start + 1, start);
        if best.is_none_or(|b| candidate < b) {
            best = Some(candidate);
        }
        i = start + 1;
    }
    best
}

/// One link's case-folded match text, computed once when the rows load so a
/// keystroke only scans.
#[derive(Debug, Clone, PartialEq)]
struct MatchKey {
    name: Vec<char>,
    uri: Vec<char>,
    uri_folded: String,
}

impl MatchKey {
    fn new(entry: &BrowseEntry) -> Self {
        let uri_folded = entry.uri.to_lowercase();
        MatchKey {
            name: entry.name.to_lowercase().chars().collect(),
            uri: uri_folded.chars().collect(),
            uri_folded,
        }
    }

    /// `(span, start)` of the better of the two fields, `None` if neither matches.
    /// The name-before-URI tie-break never changes the order between links, so it
    /// is not carried.
    fn window(&self, query: &[char]) -> Option<(usize, usize)> {
        match (
            min_window_span(query, &self.name),
            min_window_span(query, &self.uri),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Indices into `keys` of the links matching `query`, best first (the normative rule
/// in the module doc). An empty query keeps every link, in its original order.
fn rank(query: &str, keys: &[MatchKey]) -> Vec<usize> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return (0..keys.len()).collect();
    }
    let mut hits: Vec<(usize, usize, usize)> = keys
        .iter()
        .enumerate()
        .filter_map(|(ix, key)| key.window(&query).map(|(span, start)| (span, start, ix)))
        .collect();
    // Stable, so links whose case-folded URIs are equal keep their list order.
    hits.sort_by(|a, b| {
        (a.0, a.1)
            .cmp(&(b.0, b.1))
            .then_with(|| keys[a.2].uri_folded.cmp(&keys[b.2].uri_folded))
    });
    hits.into_iter().map(|(_, _, ix)| ix).collect()
}

/// What the list shows: the graph query's answer, once it lands.
#[derive(Debug, Clone, PartialEq)]
pub enum Rows {
    /// The query is in flight (the list shows its loading state).
    Loading,
    /// Every captured link, in `list_links` order.
    Loaded(Vec<BrowseEntry>),
    /// The query failed: what failed and what to do (U3).
    Failed(String),
}

impl Rows {
    /// The graph query's answer as the rows it leads to.
    pub fn from_query(result: Result<Vec<BrowseEntry>, String>) -> Rows {
        match result {
            Ok(entries) => Rows::Loaded(entries),
            Err(e) => Rows::Failed(format!(
                "Couldn't load your links: {e}. Close the browser and open it again to retry."
            )),
        }
    }

    pub fn entries(&self) -> &[BrowseEntry] {
        match self {
            Rows::Loaded(entries) => entries,
            _ => &[],
        }
    }
}

/// `List`'s delegate: a row count and one row element on demand. `List` asks only for
/// the rows its virtual list has in view, so `render_item` runs a bounded number of
/// times per frame however many links there are.
pub struct BrowserRows {
    rows: Rows,
    /// Each loaded row's match text, parallel to `rows.entries()`.
    keys: Vec<MatchKey>,
    /// The filter text the shown rows answer.
    query: String,
    /// The rows shown, as indices into `rows.entries()`, in ranked order.
    shown: Vec<usize>,
}

impl BrowserRows {
    pub fn new(rows: Rows) -> Self {
        let keys = rows.entries().iter().map(MatchKey::new).collect();
        let shown = (0..rows.entries().len()).collect();
        BrowserRows {
            rows,
            keys,
            query: String::new(),
            shown,
        }
    }

    /// Replace the rows, keeping the current filter.
    fn set_rows(&mut self, rows: Rows) {
        let query = std::mem::take(&mut self.query);
        *self = BrowserRows::new(rows);
        self.set_query(&query);
    }

    /// Re-filter for `query`.
    pub fn set_query(&mut self, query: &str) {
        self.shown = rank(query, &self.keys);
        self.query = query.to_string();
    }

    /// The shown rows, in order.
    pub fn shown(&self) -> impl Iterator<Item = &BrowseEntry> {
        let entries = self.rows.entries();
        self.shown.iter().map(move |&ix| &entries[ix])
    }

    fn shown_count(&self) -> usize {
        self.shown.len()
    }
}

impl ListDelegate for BrowserRows {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.shown_count()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let entry = self.shown().nth(ix.row)?;
        let theme = cx.theme();
        Some(
            ListItem::new(SharedString::from(format!("browser-row-{}", ix.row)))
                .h(ROW_H)
                .py_0()
                .px(spacing::S2)
                .rounded(radius::KBD)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .gap(spacing::S3)
                        .w_full()
                        .min_w_0()
                        .line_height(px(18.))
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(relative(0.45))
                                .text_size(font_size::ROW)
                                .text_color(theme.foreground)
                                .truncate()
                                .child(text!(entry.name.clone())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(font_size::META)
                                .font_family(MONO_FONT_FAMILY)
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(text!(entry.uri.clone())),
                        ),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (text, color) = match &self.rows {
            Rows::Failed(message) => (message.clone(), theme.danger),
            Rows::Loaded(entries) if !entries.is_empty() => {
                (NO_MATCH_TEXT.to_string(), theme.muted_foreground)
            }
            _ => (EMPTY_TEXT.to_string(), theme.muted_foreground),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .text_size(font_size::ROW)
            .text_color(color)
            .child(text!(text))
    }

    fn loading(&self, _cx: &App) -> bool {
        self.rows == Rows::Loading
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        cx.notify();
    }
}

/// The first-frame sample's progress (automation builds only).
#[cfg(feature = "automation")]
enum FirstFrame {
    /// Built at this instant; the first frame is not presented yet.
    Pending(Instant),
    /// The next platform frame has started; this render's deferred effect runs after
    /// its present.
    Armed(Instant),
    Recorded,
}

pub struct BrowserView {
    /// The query's answer, waiting for a render (which has the `Window` the list
    /// needs) to hand it to the list.
    pending: Option<Rows>,
    list: Option<Entity<ListState<BrowserRows>>>,
    filter: Option<Entity<InputState>>,
    /// The container that scrolls a short (unvirtualised) list.
    rows_scroll: ScrollHandle,
    /// The filter's change subscription; dropping it would stop the filtering.
    _filter_changes: Option<Subscription>,
    /// When the latest key went down, the start of a re-filter's timing.
    key_at: Option<Instant>,
    /// The in-flight graph query; dropped (cancelled) with the view.
    _load: Task<()>,
    #[cfg(feature = "automation")]
    first_frame: FirstFrame,
}

impl BrowserView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        #[cfg(feature = "automation")]
        let built_at = Instant::now();
        let load = cx.spawn(async move |this, cx| {
            let rows = cx
                .background_executor()
                .spawn(async { Rows::from_query(ubilink_app::list_links(app_state::get())) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.pending = Some(rows);
                cx.notify();
            });
        });
        // A new view times its own keystrokes (README: 0 right after it opens).
        filter_ms().clear();
        BrowserView {
            pending: None,
            list: None,
            filter: None,
            rows_scroll: ScrollHandle::new(),
            _filter_changes: None,
            key_at: None,
            _load: load,
            #[cfg(feature = "automation")]
            first_frame: FirstFrame::Pending(built_at),
        }
    }

    /// Select row `ix` and scroll it into view.
    fn select_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(list) = &self.list {
            list.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(ix)), window, cx);
                state.scroll_to_selected_item(window, cx);
            });
        }
        self.reveal(ix, cx);
    }

    /// A short list is laid out whole and scrolled by its container: keep row `ix`
    /// in that container's view. (A long list scrolls itself.)
    fn reveal(&self, ix: usize, cx: &App) {
        let count = self.row_count(cx);
        if count == 0 || count > ALL_ROWS_MAX {
            return;
        }
        let view_h = self.rows_scroll.bounds().size.height;
        let top = ROW_H * ix as f32;
        let bottom = top + ROW_H;
        let current = -self.rows_scroll.offset().y;
        // Not laid out yet (the list was long until now): put the row at the top.
        let wanted = if view_h <= px(0.) || top < current {
            top
        } else if bottom > current + view_h {
            bottom - view_h
        } else {
            current
        };
        self.rows_scroll.set_offset(point(px(0.), -wanted));
    }

    fn row_count(&self, cx: &App) -> usize {
        self.list
            .as_ref()
            .map_or(0, |l| l.read(cx).delegate().shown_count())
    }

    /// The filter text changed: re-rank the rows, select the first, and record how
    /// long it took from the key going down to the row model being ready.
    fn refilter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let started = self.key_at.take().unwrap_or_else(Instant::now);
        let (Some(filter), Some(list)) = (self.filter.clone(), self.list.clone()) else {
            return;
        };
        let query = filter.read(cx).value();
        list.update(cx, |state, _| state.delegate_mut().set_query(&query));
        filter_ms().push(started.elapsed().as_secs_f64() * 1000.0);
        list.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(0)), window, cx);
            state.scroll_to_selected_item(window, cx);
            cx.notify();
        });
        // A new result starts at its first row.
        self.rows_scroll.set_offset(point(px(0.), px(0.)));
    }

    /// Time the first frame (see the module doc): arm on the first render, record
    /// after the next frame's present.
    #[cfg(feature = "automation")]
    fn time_first_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.first_frame {
            FirstFrame::Pending(t0) => {
                self.first_frame = FirstFrame::Armed(t0);
                let this = cx.weak_entity();
                window.on_next_frame(move |_, cx| {
                    let _ = this.update(cx, |_, cx| cx.notify());
                });
            }
            FirstFrame::Armed(t0) => {
                self.first_frame = FirstFrame::Recorded;
                cx.defer(move |_| {
                    use crate::automation::store::{record_metric, MetricKind, Sample};
                    record_metric(Sample {
                        kind: MetricKind::ViewFirstFrame,
                        route: Route::Browser.as_str().into(),
                        ms: t0.elapsed().as_secs_f64() * 1000.0,
                    });
                });
            }
            FirstFrame::Recorded => {}
        }
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "automation")]
        self.time_first_frame(window, cx);

        let list = self
            .list
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut state = ListState::new(BrowserRows::new(Rows::Loading), window, cx);
                    // Selection starts on row 1 (README contract).
                    state.set_selected_index(Some(IndexPath::new(0)), window, cx);
                    state
                })
            })
            .clone();
        if let Some(rows) = self.pending.take() {
            list.update(cx, |state, cx| {
                state.delegate_mut().set_rows(rows);
                state.set_selected_index(Some(IndexPath::new(0)), window, cx);
                cx.notify();
            });
        }
        let filter = match &self.filter {
            Some(filter) => filter.clone(),
            None => {
                let filter =
                    cx.new(|cx| InputState::new(window, cx).placeholder(FILTER_PLACEHOLDER));
                // Focused from the first frame, so keys reach this view at all.
                filter.update(cx, |state, cx| state.focus(window, cx));
                self._filter_changes = Some(cx.subscribe_in(
                    &filter,
                    window,
                    |this, _, event: &FilterEvent, window, cx| {
                        if matches!(event, FilterEvent::Change) {
                            this.refilter(window, cx);
                        }
                    },
                ));
                self.filter = Some(filter.clone());
                filter
            }
        };

        let theme = cx.theme();
        div()
            // Stamp every key before the Input sees it: a re-filter is timed from here.
            .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _, _| {
                this.key_at = Some(Instant::now());
            }))
            // The focused Input binds Home/End and never propagates them; take them
            // first, in the capture phase (A11Y-NOTES.md "Keyboard").
            .capture_action(cx.listener(|this, _: &MoveEnd, window, cx| {
                let count = this.row_count(cx);
                if count > 0 {
                    this.select_row(count - 1, window, cx);
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveHome, window, cx| {
                if this.row_count(cx) > 0 {
                    this.select_row(0, window, cx);
                }
                cx.stop_propagation();
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap(GAP)
            .px(spacing::S3)
            .py(PAD_Y)
            .rounded(radius::CARD)
            .child(
                div()
                    .h(HEADER_H)
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
                            .child(text!(Route::Browser.title())),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&filter).small())),
            )
            .child({
                let shown = self.row_count(cx);
                let list = List::new(&list).scrollbar_visible(false);
                let area = div().flex_1().min_h_0();
                if shown > 0 && shown <= ALL_ROWS_MAX {
                    area.id("browser-rows")
                        .overflow_y_scroll()
                        .track_scroll(&self.rows_scroll)
                        .child(div().h(ROW_H * shown as f32).child(list))
                        .into_any_element()
                } else {
                    area.child(list).into_any_element()
                }
            })
            // The graph couldn't be opened at start-up: the list is an empty
            // in-memory graph, and the user should know why (U3).
            .when_some(app_state::startup_error(), |d, error| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(font_size::META)
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(text!(error)),
                )
            })
    }
}

/// This route's row in the router table (`views::TABLE`) builds its view from here.
pub fn build(cx: &mut App) -> AnyView {
    cx.new(BrowserView::new).into()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would re-import gpui-kit's `test` macro (pulled in
    // by this file's `use gpui_kit::*;`), which shadows Rust's `#[test]` and blows the
    // recursion limit expanding it (see shell/mod.rs's tests module for the same note).
    use super::{min_window_span, p99, state, BrowserRows, Rows, GAP, HEADER_H, PAD_Y, ROW_H};
    use std::sync::Mutex;
    use ubilink_core::{AdapterRegistry, LinkGraph, PanelBackend, UbilinkService};

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("probes/fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Rows a vertical virtual list builds for a viewport `viewport` tall with rows
    /// `row` tall and `count` rows: those crossing the viewport, plus one overdraw row
    /// past it, never past the end (gpui-base 0.7.0 `virtual_list::visible_range`).
    fn rows_built(viewport: f32, row: f32, count: usize) -> usize {
        (((viewport / row).ceil() as usize) + 1).min(count)
    }

    /// R4, headless: the 5,000-link fixture reaches the list's delegate as 5,000 rows
    /// in `list_links` order (the probe's first rows and its row 5000), and the
    /// browser's layout on the 140 pt panel builds at least the 5 rows the probe reads
    /// and at most 60.
    #[test]
    fn browser_virtualises_rows() {
        let schemes = AdapterRegistry::defaults().declared_schemes();
        let backend = Mutex::new(PanelBackend::new(
            UbilinkService::new(LinkGraph::open_in_memory().unwrap()).allow_schemes(schemes),
        ));
        backend
            .lock()
            .unwrap()
            .import_graph(&fixture("graph-5000.json"))
            .unwrap();
        let query = backend.lock().unwrap().list_links();
        let delegate = BrowserRows::new(Rows::from_query(query));
        let entries = delegate.rows.entries();
        assert_eq!(entries.len(), 5000, "every link is a row");

        let uri = |n: &str| format!("https://fixture-{n}.example.test/resource");
        let first: Vec<(&str, &str)> = entries[..5]
            .iter()
            .map(|e| (e.name.as_str(), e.uri.as_str()))
            .collect();
        let want: Vec<(String, String)> = ["00231", "04996", "00041", "02776", "01175"]
            .iter()
            .map(|n| (format!("Resource {n}"), uri(n)))
            .collect();
        assert_eq!(
            first,
            want.iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect::<Vec<_>>(),
            "rows are in list_links order"
        );
        assert_eq!(entries[4999].uri, uri("04747"), "row 5000");

        // The list's viewport on the panel, and what the virtual list builds in it.
        let (_, panel_h) = crate::shell::panel::PANEL_SIZE;
        let viewport =
            panel_h as f32 - 2. * f32::from(PAD_Y) - f32::from(HEADER_H) - f32::from(GAP);
        let built = rows_built(viewport, f32::from(ROW_H), entries.len());
        assert!(
            (5..=60).contains(&built),
            "{built} rows built in a {viewport} pt viewport"
        );

        // A failed query is a message saying what failed and what to do (U3), and no
        // rows.
        let failed = Rows::from_query(Err("the graph is locked".into()));
        match &failed {
            Rows::Failed(m) => {
                assert!(
                    m.contains("the graph is locked") && m.contains("open it again"),
                    "{m}"
                )
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(failed.entries().is_empty());
        assert!(Rows::Loading.entries().is_empty());
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn backend_with(graph: &str) -> Mutex<PanelBackend> {
        let schemes = AdapterRegistry::defaults().declared_schemes();
        let backend = Mutex::new(PanelBackend::new(
            UbilinkService::new(LinkGraph::open_in_memory().unwrap()).allow_schemes(schemes),
        ));
        backend
            .lock()
            .unwrap()
            .import_graph(&fixture(graph))
            .unwrap();
        backend
    }

    /// R5, headless: on the ranking corpus every one of the 20 golden queries ranks
    /// the table's `expect_top3` first, in order, lists only in-order subsequence
    /// matches (of the name or the URI) and drops every decoy; clearing the filter
    /// brings back every row in `list_links` order. Also the fixtures README's worked
    /// examples for the minimum window (`ab` in `aabb`) and the p99 rule.
    #[test]
    fn fuzzy_filter_ranks_subsequence() {
        assert_eq!(min_window_span(&chars("ab"), &chars("aabb")), Some((2, 1)));
        assert_eq!(
            min_window_span(&chars("gthb"), &chars("github - ubilink")),
            Some((6, 0))
        );
        assert_eq!(
            min_window_span(&chars("rddt"), &chars("https://rddt.example/thread/99231")),
            Some((4, 8))
        );
        assert_eq!(
            min_window_span(
                &chars("trgn"),
                &chars("bear://x-callback-url/open-note?id=7f3a9c21")
            ),
            None
        );
        assert_eq!(min_window_span(&chars(""), &chars("abc")), None);

        let backend = backend_with("graph-ranking.json");
        let query = backend.lock().unwrap().list_links();
        let mut rows = BrowserRows::new(Rows::from_query(query));
        let all: Vec<String> = rows.shown().map(|e| e.uri.clone()).collect();
        assert_eq!(all.len(), 22, "the whole corpus before filtering");

        let golden: serde_json::Value =
            serde_json::from_str(&fixture("ranking-golden.json")).unwrap();
        let golden = golden.as_array().unwrap();
        assert_eq!(golden.len(), 20);
        let is_subsequence = |q: &str, text: &str| {
            let mut q = q.chars().peekable();
            for c in text.to_lowercase().chars() {
                if q.peek() == Some(&c) {
                    q.next();
                }
            }
            q.peek().is_none()
        };
        for case in golden {
            let q = case["query"].as_str().unwrap();
            rows.set_query(q);
            let shown: Vec<(String, String)> = rows
                .shown()
                .map(|e| (e.name.clone(), e.uri.clone()))
                .collect();
            let uris: Vec<&str> = shown.iter().map(|(_, u)| u.as_str()).collect();
            let want: Vec<&str> = case["expect_top3"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_str().unwrap())
                .collect();
            assert_eq!(
                &uris[..want.len().min(uris.len())],
                want.as_slice(),
                "top rows for {q:?}"
            );
            for decoy in case["decoys"].as_array().unwrap() {
                assert!(
                    !uris.contains(&decoy.as_str().unwrap()),
                    "decoy {decoy} listed for {q:?}"
                );
            }
            // Exactly the subsequence matches are listed: none missing, none extra.
            for entry in rows.rows.entries() {
                let matches = is_subsequence(q, &entry.name) || is_subsequence(q, &entry.uri);
                assert_eq!(
                    uris.contains(&entry.uri.as_str()),
                    matches,
                    "{q:?} vs {} / {}",
                    entry.name,
                    entry.uri
                );
            }
        }
        rows.set_query("");
        let back: Vec<String> = rows.shown().map(|e| e.uri.clone()).collect();
        assert_eq!(
            back, all,
            "an empty filter lists every row in list_links order"
        );

        // Case-folded: upper-case typing ranks the same.
        rows.set_query("GTHB");
        assert_eq!(
            rows.shown().next().map(|e| e.uri.as_str()),
            Some("https://github.com/ubilink/ubilink")
        );

        // Nearest-rank p99: with 50 samples it is the largest; none is null.
        let samples: Vec<f64> = (1..=50).map(f64::from).collect();
        assert_eq!(p99(&samples), Some(50.0));
        assert_eq!(p99(&[]), None);
    }

    #[test]
    fn state_names_its_own_route() {
        let json = state().to_json();
        assert_eq!(json["view"], "browser");
        assert_eq!(json["phase"], "browsing");
        assert!(json["filter_samples"].is_u64(), "{json}");
        assert!(json.get("filter_p99_ms").is_some(), "{json}");
    }
}
