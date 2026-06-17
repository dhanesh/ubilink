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
            Support { app: "Finder".into(), platform: MacOS, mechanism: FilePath },
            Support { app: "Safari".into(), platform: MacOS, mechanism: AppleScript },
            Support { app: "Obsidian".into(), platform: MacOS, mechanism: UrlScheme },
            Support { app: "Obsidian".into(), platform: Windows, mechanism: UrlScheme },
            Support { app: "Obsidian".into(), platform: Linux, mechanism: UrlScheme },
            // Files are linkable everywhere via path — the universal floor.
            Support { app: "Files".into(), platform: MacOS, mechanism: FilePath },
            Support { app: "Files".into(), platform: Windows, mechanism: FilePath },
            Support { app: "Files".into(), platform: Linux, mechanism: FilePath },
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
        Capability::Unsupported { explanation, fallback }
    }

    /// Every supported (app, platform) pair — for the published matrix (O2).
    pub fn supported(&self) -> &[Support] {
        &self.rows
    }
}

fn fallback_note(fallback: &Option<LinkMechanism>) -> String {
    match fallback {
        Some(LinkMechanism::FilePath) => "If the resource is a file, you can still link it by path.".into(),
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
            Capability::Unsupported { explanation, fallback } => {
                assert!(!explanation.trim().is_empty(), "U3 violated: empty explanation");
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
                assert!(explanation.contains("MacOS"), "should name where it DOES work: {explanation}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn files_are_linkable_on_every_platform() {
        // B1's universal floor: a file is linkable everywhere.
        let m = CapabilityMatrix::baseline();
        for p in [Platform::MacOS, Platform::Windows, Platform::Linux] {
            assert_eq!(m.capability("Files", p), Capability::Supported(LinkMechanism::FilePath));
        }
    }
}
