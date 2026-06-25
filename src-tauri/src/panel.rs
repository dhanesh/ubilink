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

use crate::adapter::ResourceAdapter;
use crate::resource::ResourceIdentity;
use crate::service::UbilinkService;
use serde::Serialize;

/// One link as the panel renders it (name + uri). Serialized to the webview as JSON.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LinkView {
    pub name: String,
    pub uri: String,
}

impl From<ResourceIdentity> for LinkView {
    fn from(r: ResourceIdentity) -> LinkView {
        LinkView { name: r.name, uri: r.uri }
    }
}

/// The "ready" view model: the captured resource plus its backlinks (U5).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PanelView {
    pub resource: LinkView,
    pub backlinks: Vec<LinkView>,
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
    pub fn capture_current(&mut self, adapter: &dyn ResourceAdapter) -> Result<PanelView, String> {
        let id = self.service.capture(adapter).map_err(|e| e.message())?;
        let backlinks = self
            .service
            .backlinks(&id.uri)
            .map_err(|e| e.message())?
            .into_iter()
            .map(LinkView::from)
            .collect();
        Ok(PanelView { resource: id.into(), backlinks })
    }

    /// Expose the service for commands that link/open (kept thin on purpose).
    pub fn service_mut(&mut self) -> &mut UbilinkService {
        &mut self.service
    }

    /// The `list_links` command: every captured resource as title+uri, A→Z — the
    /// link-browser's top level (Cmd+Shift+L). The webview shows the names only.
    pub fn list_links(&self) -> Result<Vec<LinkView>, String> {
        Ok(self
            .service
            .list_resources()
            .map_err(|e| e.message())?
            .into_iter()
            .map(LinkView::from)
            .collect())
    }

    /// The `links_of` command: everything linked to `uri` (drill-in, →). Same
    /// bidirectional backlinks as the capture view, but for any resource by uri.
    pub fn links_of(&self, uri: &str) -> Result<Vec<LinkView>, String> {
        Ok(self
            .service
            .backlinks(uri)
            .map_err(|e| e.message())?
            .into_iter()
            .map(LinkView::from)
            .collect())
    }

    /// The `link_resources` command (browser "L"): link two captured resources.
    pub fn link_resources(&self, a_uri: &str, b_uri: &str, created_at: Option<i64>) -> Result<(), String> {
        self.service.link_existing(a_uri, b_uri, created_at).map_err(|e| e.message())
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

    fn tmp(dir: &std::path::Path, name: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn list_links_and_links_of_back_the_browser() {
        // The link-browser (Cmd+Shift+L): list every resource by title, and drill
        // into one to see everything linked to it.
        let dir = std::env::temp_dir().join(format!("ubilink-browse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let note = svc.capture(&FileAdapter::new(tmp(&dir, "Note.md"))).unwrap();
        let paper = svc.capture(&FileAdapter::new(tmp(&dir, "Paper.pdf"))).unwrap();
        let solo = svc.capture(&FileAdapter::new(tmp(&dir, "Solo.txt"))).unwrap(); // captured, never linked
        svc.link(&note, &paper, Some(1)).unwrap();

        let panel = PanelBackend::new(svc);
        let all = panel.list_links().unwrap();
        assert_eq!(all.len(), 3, "every CAPTURED resource is listed, including the unlinked one");
        // titles are present (the browser shows names, not raw URIs)
        assert!(all.iter().all(|l| !l.name.is_empty()));
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
        let a = svc.capture(&note).unwrap();
        let b = svc.capture(&paper).unwrap();
        svc.link(&a, &b, Some(1)).unwrap();

        let mut panel = PanelBackend::new(svc);
        let view = panel.capture_current(&note).unwrap();
        assert_eq!(view.resource.uri, a.uri);
        assert_eq!(view.backlinks.len(), 1, "U5: the note must surface its backlink");
        assert_eq!(view.backlinks[0].uri, b.uri);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_failure_becomes_an_explained_message_not_silence() {
        // U3: a permission-gated adapter with no grant yields a non-empty Err string
        // the webview can show — never an empty/silent failure.
        struct LiveAdapter;
        impl ResourceAdapter for LiveAdapter {
            fn id(&self) -> &str { "safari.macos" }
            fn needs_permission(&self) -> bool { true }
            fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> { unreachable!() }
            fn open_plan(&self, r: &ResourceIdentity) -> OpenPlan {
                OpenPlan { program: "open".into(), args: vec![r.uri.clone()] }
            }
        }
        let svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap())
            .with_permission(PermissionState::Denied);
        let mut panel = PanelBackend::new(svc);
        let err = panel.capture_current(&LiveAdapter).unwrap_err();
        assert!(!err.trim().is_empty(), "U3: empty explanation");
    }

    #[test]
    fn capabilities_command_discloses_the_supported_set() {
        // O2: the panel can surface what works where (the in-app disclosure).
        let svc = UbilinkService::new(LinkGraph::open_in_memory().unwrap());
        let panel = PanelBackend::new(svc);
        let caps = panel.capabilities();
        assert!(!caps.is_empty(), "O2: in-app capability disclosure must be non-empty");
        assert!(caps.iter().any(|s| s.app == "Safari"), "expected the baseline matrix");
        // serializable for the webview (the disclosure crosses the IPC boundary).
        assert!(serde_json::to_string(&caps).is_ok());
    }

    // ── Static markup contract (U1/U2/U3/U4) — proven without a GUI ──
    fn read_dist(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    #[test]
    fn panel_html_meets_the_accessibility_contract() {
        // U4: the panel must be announced and operable by assistive tech.
        let html = read_dist("index.html");
        assert!(html.contains("role=\"dialog\""), "U4: panel needs a dialog role");
        assert!(html.contains("aria-label"), "U4: panel needs an accessible name");
        assert!(html.contains("aria-live"), "U4/U1: status must be a live region");
        assert!(html.contains("role=\"alert\""), "U3: errors must be announced");
        assert!(html.contains("lang=\"en\""), "U4: document language declared");
    }

    #[test]
    fn panel_html_has_a_single_primary_copy_action() {
        // U2: link-in-<=2-actions hinges on ONE obvious primary control. (Onboarding
        // adds its own buttons, so we assert the Copy action is unique, not that the
        // page has exactly one button.)
        let html = read_dist("index.html");
        assert_eq!(html.matches("id=\"copy\"").count(), 1, "U2: exactly one primary Copy action");
        assert!(html.contains("aria-keyshortcuts=\"Enter\""), "U4/U2: Enter triggers copy");
    }

    #[test]
    fn panel_html_has_a_permission_onboarding_screen() {
        // S4/U3: the user is guided to grant the macOS permissions, not left guessing.
        let html = read_dist("index.html");
        assert!(html.contains("id=\"onboarding\""), "S4: onboarding screen must exist");
        assert!(html.contains("data-perm=\"accessibility\""), "must surface the Accessibility permission");
        assert!(html.contains("data-perm=\"automation\""), "must surface the Automation permission");
        assert!(html.contains("id=\"recheck\""), "must offer a re-check action");
        assert!(html.contains("data-pane=\"accessibility\""), "must offer to open the Accessibility pane");
        assert!(html.contains("id=\"grant-accessibility\""), "must offer a native Accessibility prompt");
        assert!(html.contains("id=\"open-setup\""), "permissions must be reachable from the capture view");
    }

    #[test]
    fn panel_css_provides_a_visible_focus_indicator() {
        // U4: keyboard users must see where focus is.
        let css = read_dist("panel.css");
        assert!(css.contains(":focus-visible"), "U4: needs a visible focus style");
        assert!(css.contains("prefers-reduced-motion"), "U4: spinner must honour reduced motion");
    }
}
