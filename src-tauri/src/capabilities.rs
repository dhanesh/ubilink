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

use serde::{Deserialize, Serialize};

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

    /// The v1 macOS-first baseline (B4). Extended by integrations over time (B3);
    /// also loadable from `capabilities/matrix.json`.
    pub fn baseline() -> CapabilityMatrix {
        use LinkMechanism::*;
        use Platform::*;
        CapabilityMatrix::new(vec![
            Support {
                app: "Finder".into(),
                platform: MacOS,
                mechanism: FilePath,
            },
            Support {
                app: "Safari".into(),
                platform: MacOS,
                mechanism: AppleScript,
            },
            Support {
                app: "Obsidian".into(),
                platform: MacOS,
                mechanism: UrlScheme,
            },
            Support {
                app: "Obsidian".into(),
                platform: Windows,
                mechanism: UrlScheme,
            },
            Support {
                app: "Obsidian".into(),
                platform: Linux,
                mechanism: UrlScheme,
            },
            // Files are linkable everywhere via path — the universal floor.
            Support {
                app: "Files".into(),
                platform: MacOS,
                mechanism: FilePath,
            },
            Support {
                app: "Files".into(),
                platform: Windows,
                mechanism: FilePath,
            },
            Support {
                app: "Files".into(),
                platform: Linux,
                mechanism: FilePath,
            },
        ])
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
                "Linking from \"{app}\" is not yet supported on {platform:?} \
                 (it works on {:?}). {}",
                s.platform,
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
             regenerate via the capabilities_doc_is_in_sync test. -->\n\
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
        out
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
            m.capability("Obsidian", Platform::MacOS),
            Capability::Supported(LinkMechanism::UrlScheme)
        );
        assert_eq!(
            m.capability("Safari", Platform::MacOS),
            Capability::Supported(LinkMechanism::AppleScript)
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
                    explanation.contains("MacOS"),
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
        assert!(md.contains("| Obsidian | macOS | URL scheme |"));
        assert!(md.contains("| Files | Windows | File path |"));
        // one body row per supported pair (exclude the header + the `---` separator).
        let is_data =
            |l: &&str| l.starts_with("| ") && !l.starts_with("| App ") && !l.contains("---");
        let body_rows = md.lines().filter(is_data).count();
        assert_eq!(body_rows, CapabilityMatrix::baseline().supported().len());
        // sorted: Files < Finder < Obsidian < Safari (case-insensitive) — Files first.
        let first = md.lines().find(is_data).unwrap();
        assert!(
            first.contains("Files"),
            "rows must be sorted by app: {first}"
        );
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
