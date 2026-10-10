//! The one shared [`AppState`] (final review I5; Phase 1 O2/TN4 build on it).
//!
//! `ubilink-app`'s command functions all take `&AppState` (the link-graph-backed
//! panel backend, the adapter registry, the disabled-adapter set, the data dir). The
//! GPUI shell builds exactly one, here, at start-up, and every view reads it through
//! [`get`] — never by building its own. Two reasons:
//!
//! - **One graph, one adapter set.** Four Phase 1 view tasks run in parallel; if each
//!   copied the Tauri bootstrap (`desktop/src/main.rs`'s `setup`), each view would
//!   open its own `LinkGraph` connection and its own `disabled` set, so a settings
//!   toggle (R10) would never reach capture, and a link marked in the browser (R8)
//!   would be invisible to another view's cached state.
//! - **No OS permission probe under automation.** The Tauri bootstrap calls
//!   `probe_automation_permission(&OsaScriptRunner)`, an `osascript` round trip to
//!   System Events that can raise a TCC prompt and steal focus mid-probe. An
//!   automation build skips it and starts `NotDetermined`: probes drive permissions
//!   through the `permission` fixture seam (`crate::fixtures`, spec O4) and capture
//!   through the `capture` fixture, so nothing in a probe depends on the real answer.
//!   A release build probes once at start-up, exactly as the Tauri shell did.
//!
//! The data dir comes from [`crate::paths::data_dir`] (the one resolver, O1), so a
//! probe instance's graph is its own `UBILINK_DATA_DIR/ubilink-graph.sqlite`, the file
//! `probe.py`'s `seed_graph` writes. A graph that cannot be opened does not abort the
//! menu-bar app: the state falls back to an in-memory graph and [`startup_error`]
//! carries the reason for a view to show (U3).

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use ubilink_app::{load_disabled, AppState, HotkeyReport};
use ubilink_core::{
    AdapterRegistry, CapabilityMatrix, CommandRunner, LinkGraph, PanelBackend, PermissionState,
    UbilinkService,
};

/// The graph file inside the data dir (the Tauri shell's name, so Phase 2's migration
/// is a copy, and `scripts/probe.py`'s `GRAPH_DB_NAME`).
pub const GRAPH_FILE: &str = "ubilink-graph.sqlite";

struct Shared {
    state: AppState,
    error: Option<String>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// Build the shared state now (start-up, before the event pump), so the graph opens
/// and its path is logged before any view asks. Idempotent.
pub fn init() {
    let shared = shared();
    if let Some(e) = &shared.error {
        eprintln!("[ubilink] {e}");
    }
}

/// The shared state every view uses (`app_state::get()`). Builds it on first use if
/// [`init`] has not run. Unit tests MUST NOT call it: it opens the process's real
/// data dir (no `UBILINK_DATA_DIR` under `cargo test`) and, in a release-config test
/// binary, runs the permission probe; test against [`build`] with a temp dir instead.
// Phase 1's views are the callers; until then rustc sees none.
#[allow(dead_code)]
pub fn get() -> &'static AppState {
    &shared().state
}

/// Why the on-disk graph could not be opened at start-up (the state then runs on an
/// in-memory graph), for a view to show; `None` when it opened.
// Phase 1's views are the readers; until then rustc sees no caller.
#[allow(dead_code)]
pub fn startup_error() -> Option<&'static str> {
    shared().error.as_deref()
}

fn shared() -> &'static Shared {
    SHARED.get_or_init(|| {
        let data_dir = crate::paths::data_dir();
        let permission = startup_permission(&ubilink_core::OsaScriptRunner);
        let (state, error) = build(&data_dir, permission);
        Shared { state, error }
    })
}

/// The automation permission the service starts with. An automation build never
/// runs `runner` (I5: no TCC prompt or focus theft inside a probe); a release build
/// asks it once, as the Tauri shell's `desktop/src/main.rs` did.
fn startup_permission<R: CommandRunner>(runner: &R) -> PermissionState {
    #[cfg(feature = "automation")]
    {
        let _ = runner;
        PermissionState::NotDetermined
    }
    #[cfg(not(feature = "automation"))]
    {
        ubilink_core::probe_automation_permission(runner)
    }
}

/// The Tauri bootstrap's state, minus Tauri: the graph in `data_dir`, built-in
/// adapters extended by an optional `adapters.json` there (B3), the schemes and
/// capability matrix that registry declares (S5, O2), and the persisted disabled set.
fn build(data_dir: &Path, permission: PermissionState) -> (AppState, Option<String>) {
    let mut error = None;
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        error = Some(format!(
            "couldn't create data dir {}: {e}",
            data_dir.display()
        ));
    }
    let db_path = data_dir.join(GRAPH_FILE);
    let graph = match LinkGraph::open(&db_path.to_string_lossy()) {
        Ok(g) => {
            eprintln!("[ubilink] link graph: {}", db_path.display());
            g
        }
        Err(e) => {
            error = Some(format!(
                "couldn't open link graph at {}: {e}; links captured now are not saved",
                db_path.display()
            ));
            LinkGraph::open_in_memory().expect("an in-memory SQLite graph always opens")
        }
    };

    let mut adapters = AdapterRegistry::defaults();
    let user_adapters = data_dir.join("adapters.json");
    if let Ok(json) = std::fs::read_to_string(&user_adapters) {
        match adapters.extend_from_json(&json) {
            Ok(n) => eprintln!(
                "[ubilink] loaded {n} user adapter(s) from {}",
                user_adapters.display()
            ),
            Err(e) => eprintln!(
                "[ubilink] ignoring invalid {}: {e}",
                user_adapters.display()
            ),
        }
    }
    let schemes = adapters.declared_schemes();
    let matrix = CapabilityMatrix::from_registry(&adapters);
    let disabled = load_disabled(data_dir);
    let state = AppState {
        backend: Mutex::new(PanelBackend::new(
            UbilinkService::new(graph)
                .with_permission(permission)
                .allow_schemes(schemes)
                .with_capabilities(matrix),
        )),
        adapters,
        disabled: Mutex::new(disabled),
        data_dir: data_dir.to_path_buf(),
        hotkeys: Mutex::new(HotkeyReport::default()),
    };
    (state, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ubilink-app-state-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn builds_the_graph_and_adapter_state_in_the_data_dir() {
        let dir = tmp("build");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugins-state.json"), r#"{"disabled": ["Bear"]}"#).unwrap();
        let (state, error) = build(&dir, PermissionState::NotDetermined);
        assert_eq!(error, None);
        assert!(
            dir.join(GRAPH_FILE).is_file(),
            "the graph lives in the data dir"
        );
        assert_eq!(state.data_dir, dir);
        assert!(state.disabled.lock().unwrap().contains("bear"));
        assert!(!state.adapters.apps().is_empty(), "built-in adapters load");
        // The same functions the views call work against it.
        let stats = ubilink_app::graph_stats(&state).unwrap();
        assert_eq!((stats.resources, stats.links), (0, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unopenable_graph_falls_back_and_says_why() {
        // A data dir that is a regular file: neither the dir nor the graph can exist.
        let file = tmp("blocked");
        std::fs::write(&file, b"not a directory").unwrap();
        let (state, error) = build(&file, PermissionState::NotDetermined);
        let error = error.expect("the failure is reported");
        assert!(error.contains("couldn't"), "{error}");
        assert!(
            ubilink_app::graph_stats(&state).is_ok(),
            "in-memory graph works"
        );
        std::fs::remove_file(&file).ok();
    }

    struct MustNotRun;
    impl CommandRunner for MustNotRun {
        fn run(&self, program: &str, _args: &[&str]) -> Result<String, String> {
            panic!("automation builds must not run the OS permission probe ({program})");
        }
    }

    /// I5: an automation build never runs the permission probe (no osascript, so no
    /// TCC prompt or focus theft inside a probe).
    #[cfg(feature = "automation")]
    #[test]
    fn automation_builds_skip_the_os_permission_probe() {
        assert_eq!(
            startup_permission(&MustNotRun),
            PermissionState::NotDetermined
        );
    }

    /// A release build asks the runner once and maps its answer (the Tauri rule).
    #[cfg(not(feature = "automation"))]
    #[test]
    fn release_builds_probe_the_permission_once() {
        struct Granted(std::cell::Cell<u32>);
        impl CommandRunner for Granted {
            fn run(&self, _p: &str, _a: &[&str]) -> Result<String, String> {
                self.0.set(self.0.get() + 1);
                Ok("Finder".into())
            }
        }
        let r = Granted(std::cell::Cell::new(0));
        assert_eq!(startup_permission(&r), PermissionState::Granted);
        assert_eq!(r.0.get(), 1);
        let _ = MustNotRun; // only exercised by the automation test
    }
}
