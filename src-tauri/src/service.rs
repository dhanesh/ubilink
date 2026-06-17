// Satisfies: integration of B1 (capture+API), RT-1/RT-3 (identity), RT-4/T2/U5
// (bidirectional graph + backlinks), S4/RT-2/TN7 (permission gate), RT-7/U3
// (capability matrix degradation), S5 (every captured link is validated through
// the bounded API). This is the m6-integrate facade: the wiring that makes the
// library an actual linking engine.
// See .manifold/linking-manifesto-app.md
//
// HookyService is the single entry point a UI (or the Tauri command layer) calls.
// It composes the pieces built in earlier rounds into the core user flow:
//   capture active resource (permission-gated, S4) -> validate the link through
//   the bounded API (S5) -> hook it to another resource in the graph (RT-4) ->
//   query backlinks from either end (U5). Unsupported apps degrade honestly (U3).

use crate::adapter::{capture_active, CaptureFailure, PermissionState, ResourceAdapter};
use crate::api::{ApiError, LinkApi};
use crate::capabilities::{Capability, CapabilityMatrix, Platform};
use crate::graph::LinkGraph;
use crate::resource::{Link, ResourceIdentity};

#[derive(Debug)]
pub enum ServiceError {
    /// Capture failed (permission missing / unsupported / adapter error). Carries
    /// the user-facing explanation (U3 — never silent).
    Capture(CaptureFailure),
    /// The captured link failed API validation (bad scheme / too large / rate).
    Validation(ApiError),
    /// The graph store failed.
    Store(String),
}

impl ServiceError {
    /// The message a UI shows the user (always non-empty — U3).
    pub fn message(&self) -> String {
        match self {
            ServiceError::Capture(c) => c.explanation().to_string(),
            ServiceError::Validation(a) => a.message(),
            ServiceError::Store(e) => format!("Couldn't save the link: {e}"),
        }
    }
}

pub struct HookyService {
    graph: LinkGraph,
    matrix: CapabilityMatrix,
    api: LinkApi,
    permission: PermissionState,
    platform: Platform,
}

impl HookyService {
    pub fn new(graph: LinkGraph) -> HookyService {
        HookyService {
            graph,
            matrix: CapabilityMatrix::baseline(),
            api: LinkApi::new(),
            permission: PermissionState::NotDetermined,
            platform: current_platform(),
        }
    }

    /// Set the automation permission (real detection wires `detect_automation_permission`).
    pub fn with_permission(mut self, p: PermissionState) -> HookyService {
        self.permission = p;
        self
    }

    /// Capture a link to the resource an adapter is bound to: permission gate
    /// (S4/TN7) -> bounded API validation (S5). The single "get a link" path.
    pub fn capture(&mut self, adapter: &dyn ResourceAdapter) -> Result<ResourceIdentity, ServiceError> {
        let raw = capture_active(adapter, self.permission).map_err(ServiceError::Capture)?;
        // Re-validate the adapter's output through the bounded API (defense in depth:
        // even a trusted adapter's URI is size/scheme-checked, S5/SC-1).
        self.api
            .construct_link(&raw.uri, &raw.name)
            .map(|validated| ResourceIdentity {
                // keep the richer identifiers the adapter gathered (RT-3)
                path: raw.path,
                bookmark: raw.bookmark,
                content_hash: raw.content_hash,
                ..validated
            })
            .map_err(ServiceError::Validation)
    }

    /// Hook two captured resources bidirectionally (RT-4). `created_at` supplied by
    /// the caller (the core has no clock).
    pub fn hook(&mut self, a: &ResourceIdentity, b: &ResourceIdentity, created_at: Option<i64>) -> Result<Link, ServiceError> {
        self.graph.hook(a, b, created_at).map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// The full flow: capture both ends via their adapters, then hook them.
    pub fn capture_and_hook(
        &mut self,
        active: &dyn ResourceAdapter,
        target: &dyn ResourceAdapter,
        created_at: Option<i64>,
    ) -> Result<Link, ServiceError> {
        let a = self.capture(active)?;
        let b = self.capture(target)?;
        self.hook(&a, &b, created_at)
    }

    /// Backlinks of a resource — from EITHER endpoint (U5).
    pub fn backlinks(&self, uri: &str) -> Result<Vec<ResourceIdentity>, ServiceError> {
        self.graph.backlinks(uri).map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// Capability for an app on the current platform (RT-7 / U3): drives whether
    /// linking is offered and, if not, the honest explanation.
    pub fn capability(&self, app: &str) -> Capability {
        self.matrix.capability(app, self.platform)
    }
}

fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::MacOS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{FileAdapter, OpenPlan};

    fn tmp_file(dir: &std::path::Path, name: &str, body: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn end_to_end_capture_hook_and_backlinks() {
        // The core user flow, fully exercised: capture two files, hook them, and
        // see each as the other's backlink (B1 + RT-1/3/4 + T2 + U5).
        let dir = std::env::temp_dir().join(format!("hooky-svc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = FileAdapter::new(tmp_file(&dir, "Note.md", b"note body"));
        let paper = FileAdapter::new(tmp_file(&dir, "Paper.pdf", b"%PDF paper"));

        let mut svc = HookyService::new(LinkGraph::open_in_memory().unwrap());
        let link = svc.capture_and_hook(&note, &paper, Some(1)).unwrap();
        assert_ne!(link.source_id, link.target_id);

        let note_uri = format!("file://{}", dir.join("Note.md").to_string_lossy());
        let paper_uri = format!("file://{}", dir.join("Paper.pdf").to_string_lossy());
        let from_note: Vec<String> = svc.backlinks(&note_uri).unwrap().into_iter().map(|r| r.uri).collect();
        let from_paper: Vec<String> = svc.backlinks(&paper_uri).unwrap().into_iter().map(|r| r.uri).collect();
        assert_eq!(from_note, vec![paper_uri.clone()]);
        assert_eq!(from_paper, vec![note_uri.clone()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn captured_link_carries_content_hash_through_validation() {
        // RT-3 survives the API-validation round trip (defense-in-depth doesn't
        // strip the richer identifiers).
        let dir = std::env::temp_dir().join(format!("hooky-svc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = FileAdapter::new(tmp_file(&dir, "x.md", b"abc"));
        let mut svc = HookyService::new(LinkGraph::open_in_memory().unwrap());
        let id = svc.capture(&f).unwrap();
        assert!(id.content_hash.is_some(), "content hash lost through validation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_app_capture_without_permission_degrades_not_panics() {
        // S4 / TN7 / U3: a permission-needing adapter with no grant yields a
        // user-facing ServiceError, never a silent failure or panic.
        struct LiveAdapter;
        impl ResourceAdapter for LiveAdapter {
            fn id(&self) -> &str { "safari.macos" }
            fn needs_permission(&self) -> bool { true }
            fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
                unreachable!("gate must block before capture")
            }
            fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
                OpenPlan { program: "open".into(), args: vec![r.uri.clone()] }
            }
        }
        let mut svc = HookyService::new(LinkGraph::open_in_memory().unwrap())
            .with_permission(PermissionState::Denied);
        let err = svc.capture(&LiveAdapter).unwrap_err();
        assert!(!err.message().trim().is_empty(), "U3: empty error message");
    }

    #[test]
    fn unsupported_app_capability_explains() {
        // RT-7 / U3: capability query for an unknown app returns an explanation.
        let svc = HookyService::new(LinkGraph::open_in_memory().unwrap());
        match svc.capability("TotallyUnknownApp") {
            Capability::Unsupported { explanation, .. } => assert!(explanation.contains("TotallyUnknownApp")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
