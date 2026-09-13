// Satisfies: RT-7 (capability matrix), O2 (published per-app/-platform matrix),
// U3 (honest, explained degradation — never silent failure), B1/B4 (scopes the
// "supported set" over which faithful linking is asserted), TN2 (resolution:
// B1 holds per supported adapter, not universally on day one).
// See .manifold/linking-manifesto-app.md
//
// The capability matrix is the single source of truth for WHAT linking works
// WHERE. TN2 resolved "universal vs macOS-first" by scoping B1 to the supported
// set this matrix defines. U3 requires that when a resource can't be linked the
// app explains WHY and what's possible — that explanation is computed from here,
// never a silent failure.

use crate::adapters_config::AdapterRegistry;
use serde::{Deserialize, Serialize};

/// The matrix row name for the universal file floor (`adapter::FileAdapter`).
pub const FILES_APP: &str = "Files";

/// The OS the app is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this build targets (anything not Windows/Linux reports macOS).
    pub fn current() -> Platform {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::MacOS
        }
    }
}

/// How a link to a given app's resource is obtained/opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkMechanism {
    /// App exposes a URL scheme (e.g. `obsidian://`). Most robust, cross-platform.
    UrlScheme,
    /// macOS scripting/automation (AppleScript / Apple Events).
    AppleScript,
    /// OS accessibility API to read the focused resource.
    Accessibility,
    /// Plain file path / `file://` — the universal fallback.
    FilePath,
}

/// One row of the matrix: app + platform -> mechanism (RT-7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Support {
    pub app: String,
    pub platform: Platform,
    pub mechanism: LinkMechanism,
}

/// The result of asking "can I link a resource from <app> on <platform>?".
/// On the unsupported path this is what U3 surfaces to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    /// Linking is supported via this mechanism.
    Supported(LinkMechanism),
    /// Not supported for this app/platform — `explanation` is shown to the user
    /// (U3), and `fallback` is the degraded option if one exists.
    Unsupported {
        explanation: String,
        fallback: Option<LinkMechanism>,
    },
}

pub struct CapabilityMatrix {
    rows: Vec<Support>,
}

impl CapabilityMatrix {
    pub fn new(rows: Vec<Support>) -> CapabilityMatrix {
        CapabilityMatrix { rows }
    }

    /// The matrix for the adapters that ship built in (B4): `from_registry` over
    /// `AdapterRegistry::defaults()`. This is what `docs/CAPABILITIES.md` publishes.
    pub fn baseline() -> CapabilityMatrix {
        Self::from_registry(&AdapterRegistry::defaults())
    }

    /// Derive the matrix from the adapters that actually exist (O2: honest
    /// disclosure). The registry is the single source of truth, so the matrix can
    /// neither claim an app with no adapter nor omit one that ships. The desktop
    /// passes its merged registry (defaults + the user's `adapters.json`).
    ///
    /// - Every adapter captures by running its AppleScript through `osascript`
    ///   (`ConfiguredAdapter::capture`), which exists only on macOS: one macOS row
    ///   each, and none elsewhere (off macOS `capability` explains why).
    /// - The universal file floor (`FileAdapter`, row `Files`) links a path with no
    ///   automation, so it is supported on every platform.
    pub fn from_registry(registry: &AdapterRegistry) -> CapabilityMatrix {
        let adapters = registry.specs().iter().map(|spec| Support {
            app: spec.app.clone(),
            platform: Platform::MacOS,
            mechanism: LinkMechanism::AppleScript,
        });
        let file_floor = [Platform::MacOS, Platform::Windows, Platform::Linux]
            .into_iter()
            .map(|platform| Support {
                app: FILES_APP.into(),
                platform,
                mechanism: LinkMechanism::FilePath,
            });
        CapabilityMatrix::new(adapters.chain(file_floor).collect())
    }

    /// Resolve capability for an app on a platform (RT-7). Drives U3: an
    /// unsupported answer carries a human explanation and any fallback, so the UI
    /// degrades honestly instead of failing silently.
    pub fn capability(&self, app: &str, platform: Platform) -> Capability {
        if let Some(s) = self
            .rows
            .iter()
            .find(|s| s.app.eq_ignore_ascii_case(app) && s.platform == platform)
        {
            return Capability::Supported(s.mechanism);
        }
        // Is the app supported on ANY other platform? If so the explanation says so.
        let on_other = self.rows.iter().find(|s| s.app.eq_ignore_ascii_case(app));
        // A file-backed resource can always fall back to a path link.
        let fallback = self
            .rows
            .iter()
            .any(|s| s.platform == platform && s.mechanism == LinkMechanism::FilePath)
            .then_some(LinkMechanism::FilePath);
        let explanation = match on_other {
            Some(s) => format!(
                "Linking from \"{app}\" is not supported on {}: {} {}",
                platform_name(platform),
                unavailable_reason(s),
                fallback_note(&fallback),
            ),
            None => format!(
                "\"{app}\" has no linking integration yet. {} \
                 You can add one — see plugins/README.md.",
                fallback_note(&fallback),
            ),
        };
        Capability::Unsupported {
            explanation,
            fallback,
        }
    }

    /// Every supported (app, platform) pair — for the published matrix (O2).
    pub fn supported(&self) -> &[Support] {
        &self.rows
    }

    /// Render the matrix as a published Markdown table (O2 "published in docs").
    /// `docs/CAPABILITIES.md` is GENERATED from this and drift-guarded by a test,
    /// so the published capability disclosure can never silently diverge from the
    /// code that enforces it. Rows are sorted for a stable, diffable artifact.
    pub fn to_markdown(&self) -> String {
        let mut rows = self.rows.clone();
        rows.sort_by(|a, b| {
            a.app
                .to_lowercase()
                .cmp(&b.app.to_lowercase())
                .then(platform_name(a.platform).cmp(platform_name(b.platform)))
        });
        let mut out = String::from(
            "<!-- GENERATED from CapabilityMatrix::to_markdown() (O2). Do not edit by hand;\n\
             regenerate via the ignored regenerate_capabilities_doc test. -->\n\
             # ubilink capability matrix\n\n\
             What linking works where, and by which mechanism. Unsupported app/platform\n\
             pairs degrade with an explanation in-app (U3).\n\n\
             | App | Platform | Mechanism |\n\
             | --- | --- | --- |\n",
        );
        for s in &rows {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                s.app,
                platform_name(s.platform),
                mechanism_name(s.mechanism),
            ));
        }
        // The gaps, published as plainly as the support: every (app, platform) pair
        // the app does NOT work on, with the reason the in-app explanation gives.
        let mut apps: Vec<&Support> = Vec::new();
        for s in &rows {
            if !apps.iter().any(|a| a.app.eq_ignore_ascii_case(&s.app)) {
                apps.push(s);
            }
        }
        let mut gaps = String::new();
        for a in &apps {
            for p in [Platform::Linux, Platform::MacOS, Platform::Windows] {
                if !matches!(self.capability(&a.app, p), Capability::Supported(_)) {
                    gaps.push_str(&format!(
                        "| {} | {} | {} |\n",
                        a.app,
                        platform_name(p),
                        unavailable_reason(a),
                    ));
                }
            }
        }
        if !gaps.is_empty() {
            out.push_str(
                "\n## Not supported\n\n\
                 These pairs are unsupported. The app explains why when you try, and a\n\
                 file-backed resource can still be linked by path.\n\n\
                 | App | Platform | Why |\n\
                 | --- | --- | --- |\n",
            );
            out.push_str(&gaps);
        }
        out
    }
}

/// Why an app that is supported via `s` cannot be linked on some other platform.
fn unavailable_reason(s: &Support) -> String {
    match s.mechanism {
        LinkMechanism::AppleScript => {
            "its adapter captures through macOS AppleScript (`osascript`), so it works only on macOS."
                .into()
        }
        m => format!(
            "its {} integration works only on {}.",
            mechanism_name(m),
            platform_name(s.platform)
        ),
    }
}

/// Human display name for a platform (used in the published matrix + UI).
pub fn platform_name(p: Platform) -> &'static str {
    match p {
        Platform::MacOS => "macOS",
        Platform::Windows => "Windows",
        Platform::Linux => "Linux",
    }
}

/// Human display name for a link mechanism (used in the published matrix + UI).
pub fn mechanism_name(m: LinkMechanism) -> &'static str {
    match m {
        LinkMechanism::UrlScheme => "URL scheme",
        LinkMechanism::AppleScript => "AppleScript",
        LinkMechanism::Accessibility => "Accessibility API",
        LinkMechanism::FilePath => "File path",
    }
}

fn fallback_note(fallback: &Option<LinkMechanism>) -> String {
    match fallback {
        Some(LinkMechanism::FilePath) => {
            "If the resource is a file, you can still link it by path.".into()
        }
        Some(m) => format!("A fallback is available via {m:?}."),
        None => "No fallback is available for this resource.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_app_returns_its_mechanism() {
        let m = CapabilityMatrix::baseline();
        assert_eq!(
            m.capability("Finder", Platform::MacOS),
            Capability::Supported(LinkMechanism::AppleScript)
        );
        assert_eq!(
            m.capability("safari", Platform::MacOS),
            Capability::Supported(LinkMechanism::AppleScript)
        );
        assert_eq!(
            m.capability(FILES_APP, Platform::MacOS),
            Capability::Supported(LinkMechanism::FilePath)
        );
    }

    #[test]
    fn unsupported_app_degrades_with_explanation_not_silence() {
        // U3: never a silent failure — there must be a human explanation.
        let m = CapabilityMatrix::baseline();
        let cap = m.capability("SomeRandomApp", Platform::MacOS);
        match cap {
            Capability::Unsupported {
                explanation,
                fallback,
            } => {
                assert!(
                    !explanation.trim().is_empty(),
                    "U3 violated: empty explanation"
                );
                assert!(explanation.contains("SomeRandomApp"));
                // macOS has a FilePath row, so a file fallback is offered.
                assert_eq!(fallback, Some(LinkMechanism::FilePath));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn cross_platform_gap_is_explained_honestly() {
        // Safari is macOS-only in the baseline; asking on Windows must explain that
        // (the "shallow off-macOS" pre-mortem risk, surfaced honestly).
        let m = CapabilityMatrix::baseline();
        let cap = m.capability("Safari", Platform::Windows);
        match cap {
            Capability::Unsupported { explanation, .. } => {
                assert!(
                    explanation.contains("macOS"),
                    "should name where it DOES work: {explanation}"
                );
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn files_are_linkable_on_every_platform() {
        // B1's universal floor: a file is linkable everywhere.
        let m = CapabilityMatrix::baseline();
        for p in [Platform::MacOS, Platform::Windows, Platform::Linux] {
            assert_eq!(
                m.capability("Files", p),
                Capability::Supported(LinkMechanism::FilePath)
            );
        }
    }

    #[test]
    fn markdown_table_lists_every_supported_pair_sorted() {
        // O2: the published table must contain a row for each supported pair, with
        // human-readable names, in a stable (sorted) order.
        let md = CapabilityMatrix::baseline().to_markdown();
        assert!(md.contains("| App | Platform | Mechanism |"));
        assert!(md.contains("| Safari | macOS | AppleScript |"));
        assert!(md.contains("| Bear | macOS | AppleScript |"));
        assert!(md.contains("| Files | Windows | File path |"));
        // one body row per supported pair in the supported table (exclude the header,
        // the `---` separator, and the "Not supported" section after it).
        let supported_table = md.split("\n## ").next().unwrap();
        let is_data =
            |l: &&str| l.starts_with("| ") && !l.starts_with("| App ") && !l.contains("---");
        let body_rows = supported_table.lines().filter(is_data).count();
        assert_eq!(body_rows, CapabilityMatrix::baseline().supported().len());
        // sorted case-insensitively by app: Arc sorts first among the shipped apps.
        let first = md.lines().find(is_data).unwrap();
        assert!(first.contains("Arc"), "rows must be sorted by app: {first}");
    }

    // ── O2 honesty: the matrix is DERIVED from the shipped adapters ──

    #[test]
    fn every_shipped_adapter_is_supported_on_macos() {
        // Under-claiming is dishonest too: every app with a shipped adapter captures
        // on macOS, so the published matrix must say so.
        let m = CapabilityMatrix::baseline();
        let reg = crate::adapters_config::AdapterRegistry::defaults();
        for app in reg.apps() {
            assert_eq!(
                m.capability(app, Platform::MacOS),
                Capability::Supported(LinkMechanism::AppleScript),
                "{app} ships an adapter, so it must be supported on macOS"
            );
        }
    }

    #[test]
    fn nothing_is_claimed_without_an_adapter_behind_it() {
        // Over-claiming: a row with no adapter (e.g. Obsidian) promises linking the
        // app can't do. Only shipped adapters and the universal file floor may appear.
        let m = CapabilityMatrix::baseline();
        let reg = crate::adapters_config::AdapterRegistry::defaults();
        for s in m.supported() {
            assert!(
                s.app == FILES_APP || reg.for_app(&s.app).is_some(),
                "{} on {:?} is claimed but no adapter ships for it",
                s.app,
                s.platform
            );
        }
        assert!(
            !m.supported().iter().any(|s| s.app == "Obsidian"),
            "no Obsidian adapter ships, so Obsidian must not be claimed"
        );
    }

    #[test]
    fn off_macos_only_the_file_floor_is_claimed_and_the_rest_explains_why() {
        // Every shipped adapter captures through `osascript`, which exists only on
        // macOS. Off macOS they must be unsupported, say that honestly, and offer
        // the file-path fallback.
        let m = CapabilityMatrix::baseline();
        for s in m.supported() {
            assert!(
                s.platform == Platform::MacOS || s.app == FILES_APP,
                "{} is claimed on {:?}, but its adapter needs macOS",
                s.app,
                s.platform
            );
        }
        let reg = crate::adapters_config::AdapterRegistry::defaults();
        for app in reg.apps() {
            for p in [Platform::Windows, Platform::Linux] {
                match m.capability(app, p) {
                    Capability::Unsupported {
                        explanation,
                        fallback,
                    } => {
                        assert!(explanation.contains(app), "{explanation}");
                        assert!(explanation.contains("AppleScript"), "{explanation}");
                        assert!(explanation.contains("macOS"), "{explanation}");
                        assert_eq!(fallback, Some(LinkMechanism::FilePath), "{app} on {p:?}");
                    }
                    other => panic!("{app} on {p:?}: expected Unsupported, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_user_merged_registry_is_reflected_in_the_matrix() {
        // The desktop passes its merged registry (defaults + the user's
        // adapters.json), so a user-added app is disclosed like a shipped one.
        let mut reg = crate::adapters_config::AdapterRegistry::defaults();
        reg.extend_from_json(
            r#"[{"app":"Slack","script":"x","fields":["u"],"uri":"{u}","name":"{u}"}]"#,
        )
        .unwrap();
        let m = CapabilityMatrix::from_registry(&reg);
        assert_eq!(
            m.capability("Slack", Platform::MacOS),
            Capability::Supported(LinkMechanism::AppleScript)
        );
        assert!(matches!(
            m.capability("Slack", Platform::Linux),
            Capability::Unsupported { .. }
        ));
    }

    #[test]
    fn the_published_doc_lists_the_off_macos_gaps() {
        // O2: the doc discloses where each app does NOT work, not just where it does.
        let md = CapabilityMatrix::baseline().to_markdown();
        assert!(
            md.contains("| Google Chrome | macOS | AppleScript |"),
            "{md}"
        );
        assert!(md.contains("| Safari | Windows |"), "{md}");
        assert!(md.contains("| Things3 | Linux |"), "{md}");
    }

    #[test]
    #[ignore = "generator: run with `--ignored regenerate_capabilities_doc` to (re)write docs/CAPABILITIES.md"]
    fn regenerate_capabilities_doc() {
        // The doc is a build artifact of the matrix. Run this when the matrix changes,
        // then the drift guard below proves the committed doc matches.
        let md = CapabilityMatrix::baseline().to_markdown();
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/CAPABILITIES.md");
        std::fs::write(&p, md).unwrap();
        eprintln!("wrote {}", p.display());
    }

    #[test]
    fn capabilities_doc_is_in_sync_with_the_matrix() {
        // O2 drift guard: docs/CAPABILITIES.md is GENERATED from the matrix. If the
        // matrix changes without regenerating, this fails — the published disclosure
        // can never silently diverge from the code that enforces it.
        let generated = CapabilityMatrix::baseline().to_markdown();
        let doc_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/CAPABILITIES.md");
        let committed = std::fs::read_to_string(&doc_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", doc_path.display()));
        assert_eq!(
            committed, generated,
            "docs/CAPABILITIES.md is stale — regenerate it from CapabilityMatrix::to_markdown()"
        );
    }
}
