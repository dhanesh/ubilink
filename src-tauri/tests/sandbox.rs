// Satisfies: RT-5 (binding), RT-6, S1 (invariant), T1 (boundary), TN1 (capability
// broker), TN6 (THIS is the isolation proof — manifold evidence E8/E9), SC-3.
// See .manifold/linking-manifesto-app.md
//
// End-to-end integration tests. Each spawns the REAL Node plugin host as a
// separate `node --permission` process and drives the IPC protocol, so the test
// IS the architectural proof rather than a mock of it.
//
// Run: cd src-tauri && cargo test

use hooky_core::{FocusedResource, Manifest, Session};
use serde_json::json;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../hooky/src-tauri ; repo root is its parent.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn host_script() -> PathBuf {
    repo_root().join("plugins/runtime/plugin-host.mjs")
}

fn read_plugin(example: &str) -> (Manifest, String) {
    let dir = repo_root().join("plugins/examples").join(example);
    let manifest = Manifest::from_path(&dir.join("manifest.json")).expect("valid manifest");
    // The TRUSTED core reads the bundle source from disk (it has full fs rights);
    // the source is then streamed into the sandboxed host over the pipe.
    let source = std::fs::read_to_string(dir.join("plugin.mjs")).expect("plugin source");
    (manifest, source)
}

fn focused() -> FocusedResource {
    FocusedResource {
        id: "res-1".into(),
        name: "Note.md".into(),
        uri: "obsidian://vault/Note.md".into(),
    }
}

/// TEST 1 — Fail-closed escape (manifold E8).
/// A malicious plugin attempts fs write, fs read of a secret, child_process
/// spawn, and worker_threads. Every attempt must be denied by the Node runtime.
/// Proof is INDEPENDENT of the plugin's self-report: we assert the sentinel files
/// were never created and the planted secret never leaked into the output.
#[test]
fn fail_closed_no_fs_no_child_escape() {
    let tmp = std::env::temp_dir().join(format!("hooky-spike-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let write_path = tmp.join("escaped_write.txt");
    let spawn_sentinel = tmp.join("escaped_spawn.txt");
    let secret_path = tmp.join("secret.txt");
    let secret_value = "TOP-SECRET-2a7f-link-graph-master-key";
    std::fs::write(&secret_path, secret_value).unwrap();
    // Make sure no stale sentinels exist.
    let _ = std::fs::remove_file(&write_path);
    let _ = std::fs::remove_file(&spawn_sentinel);

    let (manifest, source) = read_plugin("malicious");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn host");
    session.load(&source).unwrap();
    let probe = json!({
        "writePath": write_path.to_string_lossy(),
        "spawnSentinelPath": spawn_sentinel.to_string_lossy(),
        "secretPath": secret_path.to_string_lossy(),
    });
    let result = session.invoke(&focused(), probe).unwrap();
    eprintln!("\n[malicious plugin self-report] {}\n", serde_json::to_string_pretty(&result).unwrap());

    // --- INDEPENDENT, authoritative assertions (do not trust the plugin) ---
    assert!(
        !write_path.exists(),
        "FAIL-OPEN: plugin managed to write {write_path:?} via fs"
    );
    assert!(
        !spawn_sentinel.exists(),
        "FAIL-OPEN: plugin managed to spawn a process that wrote {spawn_sentinel:?}"
    );
    let serialized = result.to_string();
    assert!(
        !serialized.contains(secret_value),
        "FAIL-OPEN: plugin exfiltrated the secret file contents"
    );
    // The plugin tried to forge a {"t":"result","value":{"forged":true}} frame on
    // stdout to hijack the result channel; the host must have rerouted it so the
    // broker returned our REAL value, not the forged one.
    assert!(
        result.get("forged").is_none() && result["attempts"].is_array(),
        "FAIL-OPEN: plugin forged a protocol frame on stdout: {result}"
    );

    // --- corroborating: the plugin's own report must show DENIED for each ---
    let attempts = result["attempts"].as_array().expect("attempts array");
    for name in ["fs.writeFile", "fs.readFile", "child_process.spawn", "worker_threads"] {
        let a = attempts
            .iter()
            .find(|a| a["name"] == name)
            .unwrap_or_else(|| panic!("missing attempt {name}"));
        let outcome = a["outcome"].as_str().unwrap();
        assert!(
            outcome.starts_with("DENIED:"),
            "{name} was NOT denied by the runtime: {outcome}"
        );
    }
    assert!(result["leakedSecret"].is_null(), "secret was read");

    session.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// TEST 2 — Broker grant enforcement (RT-5 / RT-6 / TN1).
/// A benign plugin requests a capability that IS in its grant (open:obsidian://)
/// -> ALLOWED with a result. It also requests one NOT in its grant
/// (read:link-graph) -> DENIED.
#[test]
fn broker_allows_granted_denies_ungranted() {
    let (manifest, source) = read_plugin("benign");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn host");
    session.load(&source).unwrap();
    let result = session.invoke(&focused(), json!({})).unwrap();
    eprintln!("\n[benign plugin result] {}\n", serde_json::to_string_pretty(&result).unwrap());

    // Granted capability allowed and actually returned a result.
    assert_eq!(result["openAllowed"], true);
    assert_eq!(result["openResult"]["opened"], "obsidian://vault/Note.md");
    assert!(session.was_allowed("open:obsidian://vault/Note.md"));

    // Ungranted capability denied.
    assert_eq!(result["graphAllowed"], false);
    assert!(session.was_denied("read:link-graph"));

    session.shutdown().unwrap();
}

/// TEST 3 — No wholesale graph access (S1).
/// The only read capability ever served returns a SINGLE focused resource, never
/// a collection/graph. And read:link-graph is denied even end-to-end.
#[test]
fn no_wholesale_graph_only_focused_resource() {
    let (manifest, source) = read_plugin("benign");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn host");
    session.load(&source).unwrap();
    let result = session.invoke(&focused(), json!({})).unwrap();

    // read:focused-resource returned exactly one resource object (object, not array).
    assert_eq!(result["focusedAllowed"], true);
    let fr = &result["focusedResource"];
    assert!(fr.is_object(), "focused resource must be a single object");
    assert_eq!(fr["id"], "res-1");
    assert!(fr.get("nodes").is_none() && fr.get("edges").is_none() && !fr.is_array(),
        "a graph/collection leaked through read:focused-resource");

    // No string the plugin can send yields the graph.
    assert_eq!(result["graphAllowed"], false);

    session.shutdown().unwrap();
}

/// TEST 4 — Revoke (SC-3).
/// A capability that was ALLOWED becomes DENIED after the grant is revoked
/// mid-session — proving grants are force-invalidatable.
#[test]
fn revoke_invalidates_previously_allowed_capability() {
    let (manifest, source) = read_plugin("benign");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn host");
    session.load(&source).unwrap();

    // First run: open is allowed.
    let _ = session.invoke(&focused(), json!({})).unwrap();
    assert!(session.was_allowed("open:obsidian://vault/Note.md"));

    // Revoke the grant, then have the (still-running) plugin re-request it.
    session.broker.revoke("open:obsidian://");
    let (allowed, reason) = session.recheck("open:obsidian://vault/Note.md").unwrap();
    eprintln!("\n[after revoke] allowed={allowed} reason={reason}\n");
    assert!(!allowed, "revoked capability was still allowed");
    assert_eq!(reason, "grant revoked (SC-3)");

    session.shutdown().unwrap();
}
