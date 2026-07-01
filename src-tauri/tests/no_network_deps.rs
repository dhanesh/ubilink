// Satisfies: S2 (local-only, zero telemetry — no background egress), RT-8
// (graph/metadata stay on-device).
// See .manifold/linking-manifesto-app.md
//
// S2 guard: the trusted core must have NO outbound-network capability of its own.
// We prove it structurally — the locked dependency graph (Cargo.lock) contains no
// HTTP/network-client crate. Combined with the plugin net-jail (sandbox.rs, R1),
// there is no path by which the core or a plugin silently phones home. If anyone
// later adds `reqwest`/`hyper`/etc., this test fails loudly — egress can only be
// introduced deliberately, never by accident.
//
// Scope note: this covers the as-built core. When a future opt-in sync / UI
// network layer is added, S2 must be RE-verified with a runtime egress assertion
// (zero background connections) — that layer does not exist yet, so there is
// nothing to egress today.

use std::path::PathBuf;

/// Unambiguous outbound-network crates. Presence of any means the core could make
/// network calls — a direct S2 violation.
const FORBIDDEN: &[&str] = &[
    "reqwest",
    "hyper",
    "isahc",
    "ureq",
    "curl",
    "surf",
    "attohttpc",
    "awc",
    "actix-web",
    "axum",
    "warp",
    "rocket",
    "tonic",
    "tungstenite",
    "tokio-tungstenite",
    "websocket",
    "async-std",
    "smol",
];

#[test]
fn core_has_no_network_client_dependency() {
    let lock = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
    let text = std::fs::read_to_string(&lock).expect("Cargo.lock present");

    // Parse package names from the `[[package]] name = "..."` stanzas.
    let names: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("name = "))
        .map(|n| n.trim().trim_matches('"').to_string())
        .collect();

    let offenders: Vec<&String> = names
        .iter()
        .filter(|n| FORBIDDEN.contains(&n.as_str()))
        .collect();

    assert!(
        offenders.is_empty(),
        "S2 VIOLATION: core depends on network crate(s) {offenders:?} — the core must make zero outbound connections. \
         If this is an intentional opt-in network feature, gate it and re-verify S2 with a runtime egress test."
    );
    eprintln!(
        "[S2 guard] {} locked deps, none are network clients ✓",
        names.len()
    );
}
