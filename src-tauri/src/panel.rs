// Satisfies: U5 (the panel surfaces a captured resource's backlinks), U3 (a capture
// failure becomes an explained, non-empty message for the UI — never silent), U2
// (the view model is the minimal data the <=2-action flow needs), and the U1/U2/U4
// MARKUP CONTRACT (tests assert the static panel carries the instant-render,
// single-action, and accessibility structure). See .manifold/linking-manifesto-app.md
//
// This is the command-layer contract: the functions a Tauri `#[tauri::command]` will
// wrap (capture_current / open / dismiss). The Tauri binding itself is ENV-blocked
// (needs a webview + display) and lives behind docs/RUNBOOK-gui-panel.md; everything
// HERE is plain Rust over UbilinkService and is cargo-tested. Keeping the logic out of
// the Tauri layer is why the panel's behaviour is verifiable without a GUI.

use crate::adapter::{capture_active, ResourceAdapter};
use crate::resource::ResourceIdentity;
use crate::service::UbilinkService;
use serde::Serialize;
use std::sync::Mutex;

/// The explanation for a poisoned backend lock (a command panicked mid-update).
const POISONED: &str = "internal state was poisoned";

/// One link as the panel renders it (name + uri). Serialized to the webview as JSON.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LinkView {
    pub name: String,
    pub uri: String,
}

impl From<ResourceIdentity> for LinkView {
    fn from(r: ResourceIdentity) -> LinkView {
        LinkView {
            name: r.name,
            uri: r.uri,
        }
    }
}

/// The "ready" view model: the captured resource plus its backlinks (U5).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PanelView {
    pub resource: LinkView,
    pub backlinks: Vec<LinkView>,
}

/// One row in the link-browser: a resource (title + uri) plus how many links it
/// has — the browser shows the count on the right (0 = not linked to anything).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BrowseEntry {
    pub name: String,
    pub uri: String,
    pub links: i64,
}

impl From<(ResourceIdentity, i64)> for BrowseEntry {
    fn from((r, links): (ResourceIdentity, i64)) -> BrowseEntry {
        BrowseEntry {
            name: r.name,
            uri: r.uri,
            links,
        }
    }
}

/// Graph counts for the Settings panel.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct GraphStats {
    pub resources: i64,
    pub links: i64,
}

/// Backs the Tauri command layer. Wraps the integrated service so the panel's logic
/// is testable headlessly.
pub struct PanelBackend {
    service: UbilinkService,
}

impl PanelBackend {
    pub fn new(service: UbilinkService) -> PanelBackend {
        PanelBackend { service }
    }

    /// The `capture_current` command: capture the active resource (permission-gated,
    /// S4) and surface its existing backlinks (U5). On any failure, return the
    /// user-facing explanation (U3) as `Err` — the webview shows it, never silence.
    ///
    /// Takes the shared backend rather than `&mut self` because the adapter call
    /// (an `osascript` round trip, up to its timeout if the app stalls) must NOT
    /// hold the lock: other commands (list, open, settings) keep working meanwhile.
    /// The lock is taken only for the fast steps on either side of it.
    pub fn capture_current(
        backend: &Mutex<PanelBackend>,
        adapter: &dyn ResourceAdapter,
        now: i64,
    ) -> Result<PanelView, String> {
        let lock = || backend.lock().map_err(|_| POISONED.to_string());
        // 1. Under the lock: read the permission the S4 gate checks.
        let permission = lock()?.service.permission();
        // 2. Lock released: the slow part, the adapter's call into the target app.
        let raw = capture_active(adapter, permission).map_err(|e| e.explanation().to_string())?;
        // 3. Re-lock: validate (S5) and store the link, then read its backlinks (U5).
        let mut b = lock()?;
        let id = b.service.store_capture(raw, now).map_err(|e| e.message())?;
        let backlinks = b
            .service
            .backlinks(&id.uri)
            .map_err(|e| e.message())?
            .into_iter()
            .map(LinkView::from)
            .collect();
        Ok(PanelView {
            resource: id.into(),
            backlinks,
        })
    }

    /// Expose the service for commands that link/open (kept thin on purpose).
    pub fn service_mut(&mut self) -> &mut UbilinkService {
        &mut self.service
    }

    /// The `list_links` command: every captured resource (title, uri, link count),
    /// A→Z — the link-browser's top level (Cmd+Shift+L).
    pub fn list_links(&self) -> Result<Vec<BrowseEntry>, String> {
        Ok(self
            .service
            .list_resources_with_counts()
            .map_err(|e| e.message())?
            .into_iter()
            .map(BrowseEntry::from)
            .collect())
    }

    /// The `links_of` command: everything linked to `uri` (drill-in, →), each with
    /// its own link count so you can see where to keep navigating.
    pub fn links_of(&self, uri: &str) -> Result<Vec<BrowseEntry>, String> {
        Ok(self
            .service
            .linked_with_counts(uri)
            .map_err(|e| e.message())?
            .into_iter()
            .map(BrowseEntry::from)
            .collect())
    }

    /// The `link_resources` command (browser "L"): link two captured resources.
    pub fn link_resources(
        &self,
        a_uri: &str,
        b_uri: &str,
        created_at: Option<i64>,
    ) -> Result<(), String> {
        self.service
            .link_existing(a_uri, b_uri, created_at)
            .map_err(|e| e.message())
    }

    /// Settings (`graph_stats`): resource + link counts for the Settings panel.
    pub fn graph_stats(&self) -> Result<GraphStats, String> {
        let (resources, links) = self.service.stats().map_err(|e| e.message())?;
        Ok(GraphStats { resources, links })
    }

    /// Settings (`reset_graph`): wipe the whole graph. Returns resources removed.
    pub fn reset_graph(&mut self) -> Result<i64, String> {
        self.service.reset().map_err(|e| e.message())
    }

    /// Settings (`export_graph`): the whole graph as a JSON string.
    pub fn export_graph(&self) -> Result<String, String> {
        self.service.export().map_err(|e| e.message())
    }

    /// Settings (`import_graph`): merge a graph-export JSON in. Returns a short
    /// human summary of what was imported.
    pub fn import_graph(&mut self, json: &str) -> Result<String, String> {
        let (r, l) = self.service.import(json).map_err(|e| e.message())?;
        Ok(format!(
            "Imported {r} link{} · {l} connection{}",
            if r == 1 { "" } else { "s" },
            if l == 1 { "" } else { "s" }
        ))
    }

    /// The `open_link` command: re-validate a stored link with the capture policy
    /// and launch it via the platform's no-shell opener through `runner`.
    pub fn open_link<R: crate::live_capture::CommandRunner>(
        &self,
        runner: &R,
        uri: &str,
    ) -> Result<(), String> {
        self.service.open_link(runner, uri).map_err(|e| e.message())
    }

    /// The `capabilities` command (O2): the supported app/platform/mechanism set,
    /// serialized for the webview to disclose what works where.
    pub fn capabilities(&self) -> Vec<crate::capabilities::Support> {
        self.service.capability_overview()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{CaptureFailure, FileAdapter, OpenPlan, PermissionState};
    use crate::graph::LinkGraph;

    #[test]
    fn open_link_revalidates_with_the_configured_policy_before_launching() {
        // The desktop `open_link` command goes through here: an adapter-declared
        // scheme (bear:) opens with the platform's no-shell plan; a javascript: row
        // (e.g. planted before import validation existed) is refused, not launched.
        struct Rec(std::cell::RefCell<Vec<(String, Vec<String>)>>);
        impl crate::live_capture::CommandRunner for Rec {
            fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
                self.0.borrow_mut().push((
                    program.to_string(),
                    args.iter().map(|a| a.to_string()).collect(),
                ));
                Ok(String::new())
            }
        }
        let backend = PanelBackend::new(
            UbilinkService::new(crate::graph::LinkGraph::open_in_memory().unwrap())
                .allow_schemes(vec!["bear".to_string()]),
        );
        let rec = Rec(Default::default());
        let bear = "bear://x-callback-url/open-note?id=1&x=2";
        backend.open_link(&rec, bear).unwrap();
        let err = backend.open_link(&rec, "javascript:alert(1)").unwrap_err();
        assert!(err.contains("javascript"), "{err}");
        let expected =
            crate::adapter::open_plan_for(crate::capabilities::Platform::current(), bear);
        assert_eq!(*rec.0.borrow(), vec![(expected.program, expected.args)]);
    }

    fn tmp(dir: &std::path::Path, name: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn settings_stats_export_and_reset() {
        let dir = std::env::temp_dir().join(format!("ubilink-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let a = svc
            .capture(&FileAdapter::new(tmp(&dir, "A.md")), 1)
            .unwrap();
        let b = svc
            .capture(&FileAdapter::new(tmp(&dir, "B.md")), 1)
            .unwrap();
        svc.link(&a, &b, Some(1)).unwrap();
        let mut panel = PanelBackend::new(svc);

        let stats = panel.graph_stats().unwrap();
        assert_eq!(stats.resources, 2);
        assert_eq!(stats.links, 1);

        let json = panel.export_graph().unwrap();
        assert!(json.contains("ubilink.graph.v3") && json.contains("\"uri\":"));

        let removed = panel.reset_graph().unwrap();
        assert_eq!(removed, 2);
        assert_eq!(
            panel.graph_stats().unwrap(),
            GraphStats {
                resources: 0,
                links: 0
            }
        );
        assert!(panel.list_links().unwrap().is_empty());

        // import the earlier export back into the now-empty graph
        let summary = panel.import_graph(&json).unwrap();
        assert!(summary.contains("Imported 2 links"), "summary: {summary}");
        assert_eq!(
            panel.graph_stats().unwrap(),
            GraphStats {
                resources: 2,
                links: 1
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_links_and_links_of_back_the_browser() {
        // The link-browser (Cmd+Shift+L): list every resource by title, and drill
        // into one to see everything linked to it.
        let dir = std::env::temp_dir().join(format!("ubilink-browse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let note = svc
            .capture(&FileAdapter::new(tmp(&dir, "Note.md")), 1)
            .unwrap();
        let paper = svc
            .capture(&FileAdapter::new(tmp(&dir, "Paper.pdf")), 1)
            .unwrap();
        let solo = svc
            .capture(&FileAdapter::new(tmp(&dir, "Solo.txt")), 1)
            .unwrap(); // captured, never linked
        svc.link(&note, &paper, Some(1)).unwrap();

        let panel = PanelBackend::new(svc);
        let all = panel.list_links().unwrap();
        assert_eq!(
            all.len(),
            3,
            "every CAPTURED resource is listed, including the unlinked one"
        );
        // titles are present (the browser shows names, not raw URIs)
        assert!(all.iter().all(|l| !l.name.is_empty()));
        // link counts drive the right-hand indicator: note↔paper = 1 each, solo = 0
        assert_eq!(all.iter().find(|l| l.uri == note.uri).unwrap().links, 1);
        assert_eq!(all.iter().find(|l| l.uri == solo.uri).unwrap().links, 0);
        // drill into the note -> its one link (paper)
        let linked = panel.links_of(&note.uri).unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].uri, paper.uri);
        // the unlinked capture drills into nothing (empty, not an error)
        assert_eq!(panel.links_of(&solo.uri).unwrap().len(), 0);
        // browser "L": link solo to the note -> drill-in now surfaces it
        panel.link_resources(&solo.uri, &note.uri, Some(2)).unwrap();
        let solo_links = panel.links_of(&solo.uri).unwrap();
        assert_eq!(solo_links.len(), 1);
        assert_eq!(solo_links[0].uri, note.uri);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_current_surfaces_backlinks_for_the_resource() {
        // U5 through the panel path: a linked resource shows what it's linked to.
        let dir = std::env::temp_dir().join(format!("ubilink-panel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = FileAdapter::new(tmp(&dir, "Note.md"));
        let paper = FileAdapter::new(tmp(&dir, "Paper.pdf"));
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        // pre-link note <-> paper
        let a = svc.capture(&note, 1).unwrap();
        let b = svc.capture(&paper, 1).unwrap();
        svc.link(&a, &b, Some(1)).unwrap();

        let panel = Mutex::new(PanelBackend::new(svc));
        let view = PanelBackend::capture_current(&panel, &note, 1).unwrap();
        assert_eq!(view.resource.uri, a.uri);
        assert_eq!(
            view.backlinks.len(),
            1,
            "U5: the note must surface its backlink"
        );
        assert_eq!(view.backlinks[0].uri, b.uri);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_failure_becomes_an_explained_message_not_silence() {
        // U3: a permission-gated adapter with no grant yields a non-empty Err string
        // the webview can show — never an empty/silent failure.
        struct LiveAdapter;
        impl ResourceAdapter for LiveAdapter {
            fn id(&self) -> &str {
                "safari.macos"
            }
            fn needs_permission(&self) -> bool {
                true
            }
            fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
                unreachable!()
            }
            fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
                OpenPlan {
                    program: "open".into(),
                    args: vec![r.uri.clone()],
                }
            }
        }
        let svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap())
            .with_permission(PermissionState::Denied);
        let panel = Mutex::new(PanelBackend::new(svc));
        let err = PanelBackend::capture_current(&panel, &LiveAdapter, 1).unwrap_err();
        assert!(!err.trim().is_empty(), "U3: empty explanation");
    }

    /// An app adapter whose capture blocks until released, standing in for a
    /// stalled target app (the slow osascript call).
    struct StalledApp {
        uri: &'static str,
        entered: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl ResourceAdapter for StalledApp {
        fn id(&self) -> &str {
            "stalled"
        }
        fn needs_permission(&self) -> bool {
            false
        }
        fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(ResourceIdentity::new("Stalled", self.uri))
        }
        fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
            OpenPlan {
                program: "open".into(),
                args: vec![r.uri.clone()],
            }
        }
    }

    /// Run a capture through `StalledApp` on another thread; while it is stalled,
    /// try to take the backend lock. Returns (lock was free?, capture result).
    fn capture_while_probing_the_lock(
        backend: &Mutex<PanelBackend>,
        uri: &'static str,
    ) -> (bool, Result<PanelView, String>) {
        let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            let capture = s.spawn(move || {
                let app = StalledApp {
                    uri,
                    entered,
                    release,
                };
                PanelBackend::capture_current(backend, &app, 7)
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("capture reached the adapter");
            // Another command (here: the link browser) while the app is stalled.
            let free = backend
                .try_lock()
                .map(|b| b.list_links().is_ok())
                .unwrap_or(false);
            release_tx.send(()).unwrap();
            (free, capture.join().unwrap())
        })
    }

    #[test]
    fn backend_serves_other_commands_while_a_capture_is_in_flight() {
        // T4: one stalled app must not freeze list/open/settings for the whole
        // capture, so the backend lock is NOT held across the adapter call.
        let backend = Mutex::new(PanelBackend::new(UbilinkService::new(
            LinkGraph::open_in_memory().unwrap(),
        )));
        let (lock_was_free, result) =
            capture_while_probing_the_lock(&backend, "https://stalled.example/");
        assert!(lock_was_free, "backend lock held during the adapter call");
        // …and the capture still completes and is stored afterwards.
        assert_eq!(result.unwrap().resource.uri, "https://stalled.example/");
        let listed = backend.lock().unwrap().list_links().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].uri, "https://stalled.example/");
    }

    #[test]
    fn a_captured_link_is_still_validated_before_it_is_stored() {
        // S5: the link an adapter hands back is validated through the bounded API
        // before it reaches the graph, even though the lock was released meanwhile.
        let backend = Mutex::new(PanelBackend::new(UbilinkService::new(
            LinkGraph::open_in_memory().unwrap(),
        )));
        let (_, result) = capture_while_probing_the_lock(&backend, "javascript:alert(1)");
        let err = result.unwrap_err();
        assert!(err.contains("javascript"), "{err}");
        assert!(backend.lock().unwrap().list_links().unwrap().is_empty());
    }

    #[test]
    fn capabilities_command_discloses_the_supported_set() {
        // O2: the panel can surface what works where (the in-app disclosure).
        let svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let panel = PanelBackend::new(svc);
        let caps = panel.capabilities();
        assert!(
            !caps.is_empty(),
            "O2: in-app capability disclosure must be non-empty"
        );
        assert!(
            caps.iter().any(|s| s.app == "Safari"),
            "expected the baseline matrix"
        );
        // serializable for the webview (the disclosure crosses the IPC boundary).
        assert!(serde_json::to_string(&caps).is_ok());
    }
}
