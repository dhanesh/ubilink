//! The fixture seam (spec O4: "file dialogs take fixture paths"). Wherever the app
//! would read a real OS source — the live capture, the permission probes, the graph's
//! data dir, a save/open dialog's chosen path — it asks [`path`] first and uses the
//! fixture if one is set.
//!
//! Compiled in every build, so Phase 1 call sites need no `cfg`: in a release build
//! [`path`] is a constant `None` and the optimiser drops the branch; only an
//! `automation` build consults the store the `load_fixture` verb fills (R6). The kind
//! names live with the automation protocol, not here, so a release binary carries
//! none of them (TN1, `probe.py release-clean`).

use std::path::PathBuf;

/// The OS sources a probe can replace with a file.
// Phase 1 views are the callers (R6 wires the hook now); until then a release build
// only sees the type, so rustc would flag it as unused.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Fixture {
    /// A captured link (stands in for the frontmost app's live capture).
    Capture,
    /// Permission states (stands in for the Accessibility / Automation probes).
    Permission,
    /// The graph data directory.
    GraphDir,
    /// The path an export (save) dialog would return.
    ExportPath,
    /// The path an import (open) dialog would return.
    ImportPath,
    /// The login-item status store `SMAppService.mainApp` would be (Phase 1 R13,
    /// spec D6): read and updated by [`crate::login_item`]'s seam, so a probe never
    /// registers a real login item on the host (O4).
    LoginItem,
}

/// The fixture path for `kind`, if an automation probe loaded one. Always `None` in a
/// release build.
#[allow(dead_code)]
pub fn path(kind: Fixture) -> Option<PathBuf> {
    #[cfg(feature = "automation")]
    {
        crate::automation::store::fixture(kind)
    }
    #[cfg(not(feature = "automation"))]
    {
        let _ = kind;
        None
    }
}

#[cfg(all(test, not(feature = "automation")))]
mod tests {
    use super::*;

    #[test]
    fn release_builds_never_return_a_fixture() {
        assert_eq!(path(Fixture::Capture), None);
        assert_eq!(path(Fixture::ImportPath), None);
    }
}
