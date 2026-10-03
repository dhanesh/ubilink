//! The automation build's side stores: the opener sink (`opened`), the fixture paths
//! (`load_fixture`), the metrics ring buffer (`metrics`) and the appearance override
//! (`set_appearance`).
//!
//! Plain `std::sync::Mutex` statics rather than GPUI globals: the seams that write
//! them (`crate::opener::Opener`, `crate::fixtures::path`) are `ubilink_core`
//! `CommandRunner`s and plain functions that may run off the main thread (core work on
//! a background executor), and none of this state touches AppKit or gpui (spec R6's
//! threading rule only binds those).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;

use super::protocol::Appearance;
use crate::fixtures::Fixture;
use crate::views::Route;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding one of these leaves plain data behind, still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ── opener sink (O4) ──────────────────────────────────────────────────────

/// One open the app performed in an automation build, instead of launching it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OpenedEntry {
    pub program: String,
    pub args: Vec<String>,
    /// The last argument: the URI or System Settings pane URL that would have opened.
    pub target: String,
}

static OPENED: Mutex<Vec<OpenedEntry>> = Mutex::new(Vec::new());

// The producer is `crate::opener::Opener`, whose callers are Phase 1's views.
#[allow(dead_code)]
pub fn record_open(program: &str, args: &[&str]) {
    lock(&OPENED).push(OpenedEntry {
        program: program.into(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        target: args.last().map(|a| (*a).to_string()).unwrap_or_default(),
    });
}

pub fn opened() -> Vec<OpenedEntry> {
    lock(&OPENED).clone()
}

// ── fixtures (O4) ─────────────────────────────────────────────────────────

static FIXTURES: Mutex<BTreeMap<Fixture, PathBuf>> = Mutex::new(BTreeMap::new());

pub fn set_fixture(kind: Fixture, path: PathBuf) {
    lock(&FIXTURES).insert(kind, path);
}

pub fn fixture(kind: Fixture) -> Option<PathBuf> {
    lock(&FIXTURES).get(&kind).cloned()
}

pub fn fixtures() -> BTreeMap<Fixture, PathBuf> {
    lock(&FIXTURES).clone()
}

// ── metrics ring buffer (R5's latency numbers) ──────────────────────────────

/// What a sample measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// Task 5 (R5 latency) records these; until it lands only tests construct them.
#[allow(dead_code)]
pub enum MetricKind {
    /// Hotkey press (or `trigger_hotkey`) to the panel's first painted frame.
    HotkeyToFirstFrame,
    /// A view's construction to its first painted frame.
    ViewFirstFrame,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Sample {
    pub kind: MetricKind,
    pub route: String,
    pub ms: f64,
}

/// Fixed-capacity ring: the oldest sample drops when full, so a long probe run never
/// grows memory.
#[derive(Debug)]
pub struct Ring {
    cap: usize,
    buf: VecDeque<Sample>,
}

impl Ring {
    pub const fn new(cap: usize) -> Self {
        Ring {
            cap,
            buf: VecDeque::new(),
        }
    }

    pub fn push(&mut self, s: Sample) {
        if self.cap == 0 {
            return;
        }
        while self.buf.len() >= self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(s);
    }

    pub fn snapshot(&self) -> Vec<Sample> {
        self.buf.iter().cloned().collect()
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }
}

pub const METRICS_CAPACITY: usize = 256;

static METRICS: Mutex<Ring> = Mutex::new(Ring::new(METRICS_CAPACITY));

/// When the last hotkey (real or `trigger_hotkey`) asked for a route: Task 5 takes this
/// at the first frame and records `HotkeyToFirstFrame`.
static HOTKEY_MARK: Mutex<Option<(Route, Instant)>> = Mutex::new(None);

#[allow(dead_code)] // Task 5 (R5) is the producer.
pub fn record_metric(sample: Sample) {
    lock(&METRICS).push(sample);
}

pub fn metrics() -> (usize, Vec<Sample>) {
    let m = lock(&METRICS);
    (m.capacity(), m.snapshot())
}

pub fn mark_hotkey(route: Route, at: Instant) {
    *lock(&HOTKEY_MARK) = Some((route, at));
}

#[allow(dead_code)] // Task 5 (R5) is the consumer.
pub fn take_hotkey_mark() -> Option<(Route, Instant)> {
    lock(&HOTKEY_MARK).take()
}

// ── appearance override ─────────────────────────────────────────────────────

static APPEARANCE: Mutex<Appearance> = Mutex::new(Appearance::System);

pub fn set_appearance(a: Appearance) {
    *lock(&APPEARANCE) = a;
}

pub fn appearance() -> Appearance {
    *lock(&APPEARANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: f64) -> Sample {
        Sample {
            kind: MetricKind::ViewFirstFrame,
            route: "capture".into(),
            ms,
        }
    }

    #[test]
    fn ring_keeps_the_newest_samples_up_to_capacity() {
        let mut r = Ring::new(3);
        assert!(r.snapshot().is_empty());
        for i in 0..5 {
            r.push(sample(i as f64));
        }
        let ms: Vec<f64> = r.snapshot().iter().map(|s| s.ms).collect();
        assert_eq!(ms, [2.0, 3.0, 4.0]);
        assert_eq!(r.capacity(), 3);
        let mut zero = Ring::new(0);
        zero.push(sample(1.0));
        assert!(zero.snapshot().is_empty());
    }

    #[test]
    fn metrics_buffer_is_typed_and_serialises() {
        let (cap, _) = metrics();
        assert_eq!(cap, METRICS_CAPACITY);
        let v = serde_json::to_value(Sample {
            kind: MetricKind::HotkeyToFirstFrame,
            route: "browser".into(),
            ms: 41.5,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "hotkey_to_first_frame", "route": "browser", "ms": 41.5})
        );
    }

    #[test]
    fn fixtures_round_trip() {
        set_fixture(Fixture::GraphDir, PathBuf::from("/fx/graph"));
        assert_eq!(fixture(Fixture::GraphDir), Some(PathBuf::from("/fx/graph")));
        assert_eq!(
            crate::fixtures::path(Fixture::GraphDir),
            Some(PathBuf::from("/fx/graph")),
            "the always-compiled seam reads the store in automation builds"
        );
    }

    #[test]
    fn hotkey_mark_is_taken_once() {
        let t = Instant::now();
        mark_hotkey(Route::Settings, t);
        assert_eq!(take_hotkey_mark(), Some((Route::Settings, t)));
        assert_eq!(take_hotkey_mark(), None);
    }
}
