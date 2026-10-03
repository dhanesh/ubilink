//! The app's data directory: the ONE resolver for where ubilink-next keeps its files
//! (spec O1, R8; controller ruling R-i on Task 8).
//!
//! Every store MUST go through [`data_dir`]: the graph database
//! (`ubilink-graph.sqlite`), `plugins-state.json`, `adapters.json`, default export
//! locations, and anything Phase 1 adds. A store that builds its own path would
//! escape a probe instance's isolation and write into the user's real data.
//!
//! - `UBILINK_DATA_DIR` (absolute) wins, in every build. It is a storage location, not
//!   a control channel, so it carries no automation surface (TN1 is about the socket
//!   and verbs). `scripts/probe.py` sets it for every instance it launches, automation
//!   and release builds alike.
//! - Otherwise `~/Library/Application Support/org.ubilink.next`: the same place Tauri's
//!   `app_data_dir()` puts desktop/'s data, but under this app's own bundle id (spec
//!   D4), so the GPUI app never shares the Tauri app's (`org.dhanesh.ubilink`) data
//!   before Phase 2 decides on a migration.
//!
//! The `graph_dir` fixture (`crate::fixtures`) is separate: a probe-loaded path to a
//! prepared graph, consulted by the graph opener on top of this directory.

use std::ffi::OsString;
use std::path::PathBuf;

pub const DATA_DIR_ENV: &str = "UBILINK_DATA_DIR";
/// The bundle id the default data dir is named after (Info.plist, bundle-next.sh).
pub const BUNDLE_ID: &str = "org.ubilink.next";

/// Where this process keeps its data. Not created here: the store that first writes
/// creates it.
pub fn data_dir() -> PathBuf {
    let (dir, ignored) = resolve(std::env::var_os(DATA_DIR_ENV), std::env::var_os("HOME"));
    if let Some(bad) = ignored {
        eprintln!(
            "[ubilink-next] {DATA_DIR_ENV} ignored, must be absolute: {}",
            bad.to_string_lossy()
        );
    }
    dir
}

/// Pure resolution: the directory, plus a rejected relative override (the app's cwd is
/// not the caller's, so a relative path would silently point somewhere else).
fn resolve(overridden: Option<OsString>, home: Option<OsString>) -> (PathBuf, Option<OsString>) {
    let mut ignored = None;
    if let Some(v) = overridden.filter(|v| !v.is_empty()) {
        let p = PathBuf::from(&v);
        if p.is_absolute() {
            return (p, None);
        }
        ignored = Some(v);
    }
    // No HOME at all (a stripped launchd env) still gets a per-user-looking path
    // rather than the filesystem root.
    let home = home.map_or_else(|| PathBuf::from("/var/empty"), PathBuf::from);
    let dir = home
        .join("Library")
        .join("Application Support")
        .join(BUNDLE_ID);
    (dir, ignored)
}

/// Automation builds: an instance a probe named (`UBILINK_INSTANCE`) must also have its
/// own data dir, or it would read and write the user's real graph (O1). Refusing to
/// start makes a misconfigured harness fail loudly instead of touching real data.
#[cfg(feature = "automation")]
pub fn instance_guard(instance: Option<&OsString>, data: Option<&OsString>) -> Result<(), String> {
    match (instance, data) {
        (Some(i), None) => Err(format!(
            "UBILINK_INSTANCE={} is set but {DATA_DIR_ENV} is not: an automation instance \
             must have its own data dir (refusing to use the real one)",
            i.to_string_lossy()
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn data_dir_override_wins_when_absolute() {
        let (d, ignored) = resolve(os("/tmp/ubprobe-x/data"), os("/Users/me"));
        assert_eq!(d, PathBuf::from("/tmp/ubprobe-x/data"));
        assert_eq!(ignored, None);
    }

    #[test]
    fn data_dir_defaults_to_application_support_under_the_next_bundle_id() {
        let (d, _) = resolve(None, os("/Users/me"));
        assert_eq!(
            d,
            PathBuf::from("/Users/me/Library/Application Support/org.ubilink.next")
        );
        // Never the Tauri app's directory.
        assert!(!d.to_string_lossy().contains("org.dhanesh.ubilink"));
        // An empty override is no override.
        assert_eq!(resolve(os(""), os("/Users/me")).0, d);
    }

    #[test]
    fn relative_override_is_refused_and_reported() {
        let (d, ignored) = resolve(os("rel/data"), os("/Users/me"));
        assert_eq!(
            d,
            PathBuf::from("/Users/me/Library/Application Support/org.ubilink.next")
        );
        assert_eq!(ignored, os("rel/data"));
    }

    #[test]
    fn no_home_still_yields_an_absolute_path() {
        assert!(resolve(None, None).0.is_absolute());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn named_instance_without_a_data_dir_is_refused() {
        let inst = OsString::from("p1-shell");
        let data = OsString::from("/tmp/d");
        assert!(instance_guard(Some(&inst), None)
            .unwrap_err()
            .contains(DATA_DIR_ENV));
        assert!(instance_guard(Some(&inst), Some(&data)).is_ok());
        // The maintainer's plain automation run (no instance name) is not a probe.
        assert!(instance_guard(None, None).is_ok());
        assert!(instance_guard(None, Some(&data)).is_ok());
    }
}
