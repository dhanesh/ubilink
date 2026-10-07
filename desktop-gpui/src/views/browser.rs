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
//! Automation builds record a `view_first_frame` metric (route `browser`): from the
//! view's construction to just after its first frame's present, the same
//! next-frame-then-defer timing `shell/panel.rs` uses for the hotkey.

#[cfg(feature = "automation")]
use std::time::Instant;

use gpui_kit::component::input::{Input, InputState, MoveEnd, MoveHome};
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

/// The browser route's view-state, for the automation `state` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub view: String,
    pub phase: String,
}

/// This route's row in the router table (`views::TABLE`) reads its state from here.
pub fn state() -> super::ViewState {
    super::ViewState::Browser(State {
        view: Route::Browser.as_str().into(),
        phase: "browsing".into(),
    })
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
}

impl BrowserRows {
    pub fn new(rows: Rows) -> Self {
        BrowserRows { rows }
    }
}

impl ListDelegate for BrowserRows {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.rows.entries().len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let entry = self.rows.entries().get(ix.row)?;
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
        BrowserView {
            pending: None,
            list: None,
            filter: None,
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
    }

    fn row_count(&self, cx: &App) -> usize {
        self.list
            .as_ref()
            .map_or(0, |l| l.read(cx).delegate().rows.entries().len())
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
                state.delegate_mut().rows = rows;
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
                self.filter = Some(filter.clone());
                filter
            }
        };

        let theme = cx.theme();
        div()
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
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(List::new(&list).scrollbar_visible(false)),
            )
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
    use super::{state, BrowserRows, Rows, GAP, HEADER_H, PAD_Y, ROW_H};
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

    #[test]
    fn state_names_its_own_route() {
        let json = state().to_json();
        assert_eq!(json["view"], "browser");
        assert_eq!(json["phase"], "browsing");
    }
}
