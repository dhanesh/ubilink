// Satisfies: integration of B1 (capture+API), RT-1/RT-3 (identity), RT-4/T2/U5
// (bidirectional graph + backlinks), S4/RT-2/TN7 (permission gate), RT-7/U3
// (capability matrix degradation), S5 (every captured link is validated through
// the bounded API). This is the m6-integrate facade: the wiring that makes the
// library an actual linking engine.
// See .manifold/linking-manifesto-app.md
//
// UbilinkService is the single entry point a UI (or the Tauri command layer) calls.
// It composes the pieces built in earlier rounds into the core user flow:
//   capture active resource (permission-gated, S4) -> validate the link through
//   the bounded API (S5) -> link it to another resource in the graph (RT-4) ->
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
    /// An import was rejected (bad JSON/schema, or a row failed validation) — the
    /// graph was left unchanged.
    Import(String),
    /// Opening a link was refused (failed validation) or the OS opener failed.
    /// Carries the user-facing explanation.
    Open(String),
}

impl ServiceError {
    /// The message a UI shows the user (always non-empty — U3).
    pub fn message(&self) -> String {
        match self {
            ServiceError::Capture(c) => c.explanation().to_string(),
            ServiceError::Validation(a) => a.message(),
            ServiceError::Store(e) => format!("Couldn't save the link: {e}"),
            ServiceError::Import(e) => format!("Couldn't import: {e}"),
            ServiceError::Open(e) => e.clone(),
        }
    }
}

pub struct UbilinkService {
    graph: LinkGraph,
    matrix: CapabilityMatrix,
    api: LinkApi,
    permission: PermissionState,
    platform: Platform,
}

impl UbilinkService {
    pub fn new(graph: LinkGraph) -> UbilinkService {
        UbilinkService {
            graph,
            matrix: CapabilityMatrix::baseline(),
            api: LinkApi::new(),
            permission: PermissionState::NotDetermined,
            platform: current_platform(),
        }
    }

    /// Set the automation permission (real detection wires `detect_automation_permission`).
    pub fn with_permission(mut self, p: PermissionState) -> UbilinkService {
        self.permission = p;
        self
    }

    /// Trust the URI schemes that registered adapters declare (B3 + S5): an adapter
    /// producing `bear://…` links means `bear:` is an expected, safe scheme. Keeps
    /// the bounded API an allowlist while letting config-added apps work.
    pub fn allow_schemes(mut self, schemes: impl IntoIterator<Item = String>) -> UbilinkService {
        for s in schemes {
            self.api.allow(&s);
        }
        self
    }

    /// Capture a link to the resource an adapter is bound to: permission gate
    /// (S4/TN7) -> bounded API validation (S5). The single "get a link" path.
    /// `now` (ms, caller-supplied — core has no clock) stamps the capture time so
    /// re-capturing a resource resurfaces it to the top of the list.
    pub fn capture(
        &mut self,
        adapter: &dyn ResourceAdapter,
        now: i64,
    ) -> Result<ResourceIdentity, ServiceError> {
        let raw = capture_active(adapter, self.permission).map_err(ServiceError::Capture)?;
        // Re-validate the adapter's output through the bounded API (defense in depth:
        // even a trusted adapter's URI is size/scheme-checked, S5/SC-1).
        let id = self
            .api
            .construct_link(&raw.uri, &raw.name)
            .map(|validated| ResourceIdentity {
                // keep the richer identifiers the adapter gathered (RT-3)
                path: raw.path,
                bookmark: raw.bookmark,
                content_hash: raw.content_hash,
                ..validated
            })
            .map_err(ServiceError::Validation)?;
        // Persist the capture so the link-browser (Cmd+Shift+L) can list every link
        // you've grabbed. Upsert keyed by URI — re-capturing bumps updated_at.
        self.graph
            .upsert_resource(&id, now)
            .map_err(|e| ServiceError::Store(e.to_string()))?;
        Ok(id)
    }

    /// Link two captured resources bidirectionally (RT-4). `created_at` supplied by
    /// the caller (the core has no clock).
    pub fn link(
        &mut self,
        a: &ResourceIdentity,
        b: &ResourceIdentity,
        created_at: Option<i64>,
    ) -> Result<Link, ServiceError> {
        self.graph
            .link(a, b, created_at)
            .map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// The full flow: capture both ends via their adapters, then link them.
    pub fn capture_and_link(
        &mut self,
        active: &dyn ResourceAdapter,
        target: &dyn ResourceAdapter,
        created_at: Option<i64>,
    ) -> Result<Link, ServiceError> {
        let now = created_at.unwrap_or(0);
        let a = self.capture(active, now)?;
        let b = self.capture(target, now)?;
        self.link(&a, &b, created_at)
    }

    /// Backlinks of a resource — from EITHER endpoint (U5).
    pub fn backlinks(&self, uri: &str) -> Result<Vec<ResourceIdentity>, ServiceError> {
        self.graph
            .backlinks(uri)
            .map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// Every captured resource (the link-browser's top level, Cmd+Shift+L).
    pub fn list_resources(&self) -> Result<Vec<ResourceIdentity>, ServiceError> {
        self.graph
            .list_resources()
            .map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// Link two already-captured resources by uri (the browser's "L" action).
    pub fn link_existing(
        &self,
        a_uri: &str,
        b_uri: &str,
        created_at: Option<i64>,
    ) -> Result<(), ServiceError> {
        self.graph
            .link_existing(a_uri, b_uri, created_at)
            .map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// Settings: counts, destructive reset, and JSON export of the whole graph.
    pub fn stats(&self) -> Result<(i64, i64), ServiceError> {
        let r = self
            .graph
            .resource_count()
            .map_err(|e| ServiceError::Store(e.to_string()))?;
        let l = self
            .graph
            .link_count()
            .map_err(|e| ServiceError::Store(e.to_string()))?;
        Ok((r, l))
    }
    pub fn reset(&mut self) -> Result<i64, ServiceError> {
        self.graph
            .clear()
            .map_err(|e| ServiceError::Store(e.to_string()))
    }
    pub fn export(&self) -> Result<String, ServiceError> {
        self.graph
            .export_json()
            .map_err(|e| ServiceError::Store(e.to_string()))
    }
    /// Settings → Import. Every row is validated with the SAME policy capture uses
    /// (S5/SC-1) before anything is written; one bad row rejects the whole file.
    pub fn import(&mut self, json: &str) -> Result<(usize, usize), ServiceError> {
        let policy = self.api.policy();
        self.graph
            .import_json(json, |uri, name| {
                policy.validate(uri, name).map_err(|e| e.message())
            })
            .map_err(ServiceError::Import)
    }

    /// Open a stored link: re-validate it with the SAME policy capture and import
    /// use (defence in depth for rows written before validation existed), then
    /// launch it with the platform's no-shell, no-option opener through `runner`.
    pub fn open_link<R: crate::live_capture::CommandRunner>(
        &self,
        runner: &R,
        uri: &str,
    ) -> Result<(), ServiceError> {
        crate::adapter::open_uri(runner, self.api.policy(), self.platform, uri)
            .map_err(ServiceError::Open)
    }

    /// Every captured resource with its link count (browser list).
    pub fn list_resources_with_counts(&self) -> Result<Vec<(ResourceIdentity, i64)>, ServiceError> {
        self.graph
            .list_resources_with_counts()
            .map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// Backlinks of `uri`, each with its own link count (browser drill-in).
    pub fn linked_with_counts(
        &self,
        uri: &str,
    ) -> Result<Vec<(ResourceIdentity, i64)>, ServiceError> {
        self.graph
            .linked_with_counts(uri)
            .map_err(|e| ServiceError::Store(e.to_string()))
    }

    /// Capability for an app on the current platform (RT-7 / U3): drives whether
    /// linking is offered and, if not, the honest explanation.
    pub fn capability(&self, app: &str) -> Capability {
        self.matrix.capability(app, self.platform)
    }

    /// The full supported set, for in-app capability disclosure (O2 "surfaced
    /// in-app"). The UI renders this so users see what works where before they hit
    /// an unsupported app.
    pub fn capability_overview(&self) -> Vec<crate::capabilities::Support> {
        self.matrix.supported().to_vec()
    }
}

fn current_platform() -> Platform {
    Platform::current()
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
    fn end_to_end_capture_link_and_backlinks() {
        // The core user flow, fully exercised: capture two files, link them, and
        // see each as the other's backlink (B1 + RT-1/3/4 + T2 + U5).
        let dir = std::env::temp_dir().join(format!("ubilink-svc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = FileAdapter::new(tmp_file(&dir, "Note.md", b"note body"));
        let paper = FileAdapter::new(tmp_file(&dir, "Paper.pdf", b"%PDF paper"));

        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let link = svc.capture_and_link(&note, &paper, Some(1)).unwrap();
        assert_ne!(link.source_id, link.target_id);

        let note_uri = format!("file://{}", dir.join("Note.md").to_string_lossy());
        let paper_uri = format!("file://{}", dir.join("Paper.pdf").to_string_lossy());
        let from_note: Vec<String> = svc
            .backlinks(&note_uri)
            .unwrap()
            .into_iter()
            .map(|r| r.uri)
            .collect();
        let from_paper: Vec<String> = svc
            .backlinks(&paper_uri)
            .unwrap()
            .into_iter()
            .map(|r| r.uri)
            .collect();
        assert_eq!(from_note, vec![paper_uri.clone()]);
        assert_eq!(from_paper, vec![note_uri.clone()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn captured_link_carries_content_hash_through_validation() {
        // RT-3 survives the API-validation round trip (defense-in-depth doesn't
        // strip the richer identifiers).
        let dir = std::env::temp_dir().join(format!("ubilink-svc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = FileAdapter::new(tmp_file(&dir, "x.md", b"abc"));
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let id = svc.capture(&f, 1).unwrap();
        assert!(
            id.content_hash.is_some(),
            "content hash lost through validation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_app_capture_without_permission_degrades_not_panics() {
        // S4 / TN7 / U3: a permission-needing adapter with no grant yields a
        // user-facing ServiceError, never a silent failure or panic.
        struct LiveAdapter;
        impl ResourceAdapter for LiveAdapter {
            fn id(&self) -> &str {
                "safari.macos"
            }
            fn needs_permission(&self) -> bool {
                true
            }
            fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
                unreachable!("gate must block before capture")
            }
            fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
                OpenPlan {
                    program: "open".into(),
                    args: vec![r.uri.clone()],
                }
            }
        }
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap())
            .with_permission(PermissionState::Denied);
        let err = svc.capture(&LiveAdapter, 1).unwrap_err();
        assert!(!err.message().trim().is_empty(), "U3: empty error message");
    }

    /// A crafted v3 export: one valid row, one malicious row, and a link between them.
    fn crafted_export(bad_name: &str, bad_uri: &str) -> String {
        serde_json::json!({
            "schema": "ubilink.graph.v3",
            "resources": [
                {"guid": "g-ok", "name": "Fine", "uri": "https://ok.example/"},
                {"guid": "g-bad", "name": bad_name, "uri": bad_uri}
            ],
            "links": [{"a": "g-ok", "b": "g-bad", "created_at": 1}]
        })
        .to_string()
    }

    #[test]
    fn import_rejects_a_crafted_export_atomically_naming_the_row() {
        // SC-1 at the import boundary: a shared export file must not be able to plant
        // links that capture would have refused. One bad row rejects the WHOLE file
        // (the valid row and the link are not written) and the error names the row.
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        svc.graph
            .upsert_resource(
                &ResourceIdentity::new("Existing", "https://example.com/"),
                1,
            )
            .unwrap();
        let oversized = format!("https://x/{}", "a".repeat(crate::api::MAX_INPUT_BYTES));
        let cases: [(&str, &str, &str); 7] = [
            ("javascript", "Evil", "javascript:alert(1)"),
            ("data", "Evil", "data:text/html,<script>alert(1)</script>"),
            ("smb (not allowlisted)", "Evil", "smb://evil.example/share"),
            ("leading dash", "Evil", "-a"),
            ("bidi in uri", "Evil", "https://x/\u{202E}fdp.exe"),
            ("control char in name", "Evil\u{0007}", "https://x/a"),
            ("oversized uri", "Evil", &oversized),
        ];
        for (label, name, uri) in cases {
            let err = svc
                .import(&crafted_export(name, uri))
                .expect_err(label)
                .message();
            assert!(err.contains("resources[1]"), "{label}: {err}");
            assert_eq!(
                svc.stats().unwrap(),
                (1, 0),
                "{label}: the graph must be left unchanged"
            );
        }
    }

    #[test]
    fn own_export_round_trips_through_validated_import() {
        // Regression guard: whatever capture accepted, import must accept too —
        // including config-declared schemes (bear:) and awkward names.
        struct BearNote;
        impl ResourceAdapter for BearNote {
            fn id(&self) -> &str {
                "bear"
            }
            fn needs_permission(&self) -> bool {
                false
            }
            fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
                Ok(ResourceIdentity::new(
                    "Idea \"quoted\" \u{05E9}\u{200F}",
                    "bear://x-callback-url/open-note?id=1&title=a%20b",
                ))
            }
            fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
                OpenPlan {
                    program: "open".into(),
                    args: vec![r.uri.clone()],
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("ubilink-svc-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = FileAdapter::new(tmp_file(&dir, "My Note.md", b"x"));

        let schemes = || vec!["bear".to_string()];
        let mut svc =
            UbilinkService::new(LinkGraph::open_in_memory().unwrap()).allow_schemes(schemes());
        svc.capture_and_link(&file, &BearNote, Some(5)).unwrap();
        let json = svc.export().unwrap();

        let mut fresh =
            UbilinkService::new(LinkGraph::open_in_memory().unwrap()).allow_schemes(schemes());
        assert_eq!(fresh.import(&json).unwrap(), (2, 1));
        assert_eq!(fresh.stats().unwrap(), (2, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsupported_app_capability_explains() {
        // RT-7 / U3: capability query for an unknown app returns an explanation.
        let svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        match svc.capability("TotallyUnknownApp") {
            Capability::Unsupported { explanation, .. } => {
                assert!(explanation.contains("TotallyUnknownApp"))
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
