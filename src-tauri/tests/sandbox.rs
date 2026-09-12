// Satisfies: RT-5 (binding), RT-6, S1 (invariant), T1 (boundary), TN1 (capability
// broker), TN6 (THIS is the isolation proof — manifold evidence E8/E9), SC-3.
// See .manifold/linking-manifesto-app.md
//
// End-to-end integration tests. Each spawns the REAL Node plugin host as a
// separate `node --permission` process and drives the IPC protocol, so the test
// IS the architectural proof rather than a mock of it.
//
// Run: cd src-tauri && cargo test

use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ubilink_core::{FocusedResource, Manifest, Session};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../ubilink/src-tauri ; repo root is its parent.
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
    let tmp = std::env::temp_dir().join(format!("ubilink-spike-{}", std::process::id()));
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
    let result = session.invoke(probe).unwrap();
    eprintln!(
        "\n[malicious plugin self-report] {}\n",
        serde_json::to_string_pretty(&result).unwrap()
    );

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
    for name in [
        "fs.writeFile",
        "fs.readFile",
        "child_process.spawn",
        "worker_threads",
    ] {
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
    let result = session.invoke(json!({})).unwrap();
    eprintln!(
        "\n[benign plugin result] {}\n",
        serde_json::to_string_pretty(&result).unwrap()
    );

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
    let result = session.invoke(json!({})).unwrap();

    // read:focused-resource returned exactly one resource object (object, not array).
    assert_eq!(result["focusedAllowed"], true);
    let fr = &result["focusedResource"];
    assert!(fr.is_object(), "focused resource must be a single object");
    assert_eq!(fr["id"], "res-1");
    assert!(
        fr.get("nodes").is_none() && fr.get("edges").is_none() && !fr.is_array(),
        "a graph/collection leaked through read:focused-resource"
    );

    // No string the plugin can send yields the graph.
    assert_eq!(result["graphAllowed"], false);

    session.shutdown().unwrap();
}

/// TEST 5 — Network is jailed (S1 / DECISIONS D2 -> closed).
/// `node --permission` does NOT block outbound network; the macOS sandbox-exec
/// deny-network profile does. We start a REAL local TCP listener and have the
/// malicious plugin try to connect to it. When the jail is active the listener
/// must receive ZERO connections and the plugin's attempt must be DENIED.
#[test]
fn net_is_jailed_when_supported() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let accepts = Arc::new(AtomicUsize::new(0));
    let accepts2 = accepts.clone();
    listener.set_nonblocking(false).unwrap();
    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            match listener.accept() {
                Ok(_) => {
                    accepts2.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    });

    let (manifest, source) = read_plugin("malicious");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn host");
    let jailed = session.net_jailed();
    session.load(&source).unwrap();
    let probe = json!({
        "writePath": std::env::temp_dir().join("x").to_string_lossy(),
        "spawnSentinelPath": std::env::temp_dir().join("y").to_string_lossy(),
        "secretPath": std::env::temp_dir().join("z").to_string_lossy(),
        "netPort": port,
    });
    let result = session.invoke(probe).unwrap();
    session.shutdown().unwrap();
    let _ = handle.join();

    let attempts = result["attempts"].as_array().expect("attempts");
    let net = attempts.iter().find(|a| a["name"] == "net.connect");

    // On macOS the jail is REQUIRED: a silently-skipped jail must fail the proof,
    // not pass it vacuously.
    if cfg!(target_os = "macos") {
        assert!(
            jailed,
            "the macOS OS jail must be active (sandbox-exec probe failed)"
        );
    }
    if jailed {
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            0,
            "FAIL-OPEN: jailed plugin still connected to the listener"
        );
        let outcome = net.expect("net attempt")["outcome"].as_str().unwrap();
        assert!(
            outcome.starts_with("DENIED:"),
            "net was not denied under the jail: {outcome}"
        );
        eprintln!("[net jail] active — connection denied ({outcome})");
    } else {
        // Non-macOS / no sandbox-exec: net jail not wired yet (documented gap).
        eprintln!("[net jail] NOT active on this platform — net containment is a known follow-up (Linux seccomp / Windows AppContainer)");
    }
}

/// TEST 6 — Hung plugin is killed (S5 / DECISIONS D4).
/// A plugin whose run() never returns must NOT hang the host forever: the session
/// times out and kills the child, returning an error rather than blocking.
#[test]
fn hung_plugin_is_killed() {
    let (manifest, source) = read_plugin("hang");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn host");
    session.set_timeout(Duration::from_millis(800)); // short, for the test
    session.load(&source).unwrap();
    let start = std::time::Instant::now();
    let res = session.invoke(json!({}));
    let elapsed = start.elapsed();
    assert!(
        res.is_err(),
        "hung plugin should have timed out, got: {res:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout took too long: {elapsed:?}"
    );
    eprintln!(
        "[exec timeout] hung plugin killed after {elapsed:?}: {}",
        res.unwrap_err()
    );
}

// ---------------------------------------------------------------------------
// Sandbox hardening (hostile-plugin fixture: plugins/examples/hostile).
// Each test below reproduces an attack a reviewer confirmed against the real
// host; see DECISIONS.md D1/D3/D4.
// ---------------------------------------------------------------------------

fn hostile() -> (Manifest, String) {
    read_plugin("hostile")
}

/// Run `f` on a helper thread and give up after `limit`. A test for a DoS
/// fix must FAIL (not hang the suite) when the fix is missing.
fn run_bounded<T: Send + 'static>(
    limit: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok()
}

/// Is there still a process table entry for `pid`? A zombie (killed but never
/// waited on) still has one, so this proves "killed AND reaped".
fn pid_exists(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// ATTACK 1 — Frame forgery. Every way a plugin was shown to inject a
/// `{"t":"result"}` frame (stream-prototype write, raw fd-1 write, replacing
/// the global `JSON.stringify` the host serializes with, polluting
/// `Object.prototype.toJSON`, scraping the V8 heap for the session secret,
/// and scanning argv/env/stdin for it) must be REJECTED by the core: it returns
/// the plugin's real value, never the forged one.
#[test]
fn forged_frames_are_rejected() {
    let mut failures = Vec::new();
    for vector in [
        "proto-write",
        "fd-write",
        "stringify",
        "tojson",
        "heap",
        "scan",
    ] {
        let (manifest, source) = hostile();
        let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
        session.set_timeout(Duration::from_secs(5));
        session.load(&source).unwrap();
        let result = match session.invoke(json!({ "mode": "forge", "vector": vector })) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("[{vector}] invoke failed: {e}"));
                continue;
            }
        };
        eprintln!("[forge:{vector}] core returned {result}");
        if result.get("forged").is_some() || result["real"] != true {
            failures.push(format!(
                "FAIL-OPEN [{vector}]: core accepted a forged frame: {result}"
            ));
        }
        let heap = result["notes"]["heap"].as_str().unwrap_or("");
        if vector == "heap" && !heap.starts_with("DENIED") {
            failures.push(format!(
                "[heap] plugin could snapshot the V8 heap (would expose the nonce): {heap}"
            ));
        }
        if vector == "scan" && result["notes"]["candidates"] != 0 {
            failures.push(format!(
                "[scan] nonce-shaped secret visible via argv/env/stdin: {result}"
            ));
        }
        session.shutdown().unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const PLANTED_VAR: &str = "UBILINK_PLANTED_SECRET";
const PLANTED_VALUE: &str = "planted-secret-9c1e";
const ENV_PROBE_MARKER: &str = "UBILINK_ENV_PROBE_CHILD";

/// ATTACK 3 — Environment disclosure + NODE_OPTIONS re-grant. Re-executes this
/// test binary with a planted secret and `NODE_OPTIONS=--allow-fs-read=*` in
/// its environment (so the test never mutates its own env while other tests
/// spawn processes); the inner test asserts the plugin saw neither.
#[test]
fn child_env_is_scrubbed() {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args([
            "env_probe_inner",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ENV_PROBE_MARKER, "1")
        .env(PLANTED_VAR, PLANTED_VALUE)
        .env("NODE_OPTIONS", "--allow-fs-read=*")
        .output()
        .expect("re-exec test binary");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && text.contains("1 passed"),
        "env probe failed:\n{text}"
    );
}

/// Inner half of `child_env_is_scrubbed` (a no-op unless re-executed by it).
#[test]
fn env_probe_inner() {
    if std::env::var_os(ENV_PROBE_MARKER).is_none() {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("ubilink-env-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let secret_path = tmp.join("secret.txt");
    std::fs::write(&secret_path, "FS-SECRET-5d2b").unwrap();

    let (manifest, source) = hostile();
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
    session.load(&source).unwrap();
    let r = session
        .invoke(json!({ "mode": "env", "plantedVar": PLANTED_VAR, "secretPath": secret_path.to_string_lossy() }),
        )
        .unwrap();
    session.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("[env probe] {r}");

    assert!(
        r["planted"].is_null(),
        "plugin saw the core's env secret: {r}"
    );
    assert!(
        r["nodeOptions"].is_null(),
        "NODE_OPTIONS reached the plugin host: {r}"
    );
    assert_eq!(
        r["fsReadGranted"], false,
        "inherited NODE_OPTIONS re-granted fs read: {r}"
    );
    assert!(
        r["secret"].is_null(),
        "plugin read a file via an inherited grant: {r}"
    );
    let keys: Vec<&str> = r["keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|k| k.as_str())
        // macOS CoreFoundation injects this into every process; it is not ours.
        .filter(|k| *k != "__CF_USER_TEXT_ENCODING")
        .collect();
    assert!(keys.is_empty(), "plugin inherited env vars: {keys:?}");
}

/// ATTACK 4 — Signals. `node --permission` does not gate `process.kill`; the
/// macOS jail must deny signalling any process other than the plugin itself.
#[test]
fn plugin_cannot_signal_the_core() {
    let (manifest, source) = hostile();
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
    let jailed = session.net_jailed();
    session.load(&source).unwrap();
    let r = session.invoke(json!({ "mode": "signal" })).unwrap();
    session.shutdown().unwrap();
    eprintln!("[signal probe] {r}");
    if cfg!(target_os = "macos") {
        assert!(
            jailed,
            "the macOS OS jail must be active (sandbox-exec probe failed)"
        );
    }
    if jailed {
        let o = r["killParent"].as_str().unwrap();
        assert!(
            o.starts_with("DENIED:EPERM"),
            "plugin could signal the core: {o}"
        );
    } else {
        eprintln!(
            "[signal] OS jail not active on this platform — signal containment is a follow-up"
        );
    }
}

/// Native file writes that bypass the Node permission model: `trace_events`
/// writes `node_trace.N.log` into the host's cwd, `process.report` writes a
/// report file. Neither may create a file.
#[test]
fn plugin_cannot_write_files_natively() {
    let tmp = std::env::temp_dir().join(format!("ubilink-native-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let report_path = tmp.join("report.json");
    let cwd_trace = std::env::current_dir().unwrap().join("node_trace.1.log");
    let trace_existed = cwd_trace.exists();

    let (manifest, source) = hostile();
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
    session.set_timeout(Duration::from_secs(3));
    session.load(&source).unwrap();
    let r = session
        .invoke(json!({ "mode": "native-write", "reportPath": report_path.to_string_lossy() }));
    drop(session);
    let trace_created = !trace_existed && cwd_trace.exists();
    if trace_created {
        let _ = std::fs::remove_file(&cwd_trace);
    }
    let report_created = report_path.exists();
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("[native write] {r:?}");

    assert!(
        !trace_created,
        "FAIL-OPEN: trace_events wrote {cwd_trace:?}"
    );
    assert!(
        !report_created,
        "FAIL-OPEN: process.report wrote {report_path:?}"
    );
    let r = r.expect("invoke");
    assert!(r["trace"].as_str().unwrap().starts_with("DENIED"), "{r}");
}

/// ATTACK 5a — A chatty plugin that keeps sending legitimate traffic under the
/// per-message timeout must still be killed at an OVERALL deadline.
#[test]
fn chatty_plugin_is_killed_at_overall_deadline() {
    let done = run_bounded(Duration::from_secs(8), || {
        let (manifest, source) = hostile();
        let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
        session.set_timeout(Duration::from_millis(800));
        session.load(&source).unwrap();
        let start = std::time::Instant::now();
        let res = session.invoke(json!({ "mode": "chatty" }));
        (res.map_err(|e| e.kind()), start.elapsed())
    });
    let (res, elapsed) =
        done.expect("FAIL-OPEN: chatty plugin was never killed (no overall deadline)");
    assert_eq!(res.unwrap_err(), std::io::ErrorKind::TimedOut);
    assert!(
        elapsed < Duration::from_secs(3),
        "deadline overshot: {elapsed:?}"
    );
}

/// ATTACK 5b — A plugin that stops reading stdin while flooding requests makes
/// the core's pipe WRITE block; the deadline must still unblock it.
#[test]
fn flooding_plugin_cannot_block_core_writes() {
    let done = run_bounded(Duration::from_secs(8), || {
        let (manifest, source) = hostile();
        let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
        session.set_timeout(Duration::from_millis(800));
        session.load(&source).unwrap();
        let start = std::time::Instant::now();
        let res = session.invoke(json!({ "mode": "flood" }));
        (res.map_err(|e| e.kind()), start.elapsed())
    });
    let (res, elapsed) =
        done.expect("FAIL-OPEN: core blocked forever writing to a flooding plugin");
    assert_eq!(res.unwrap_err(), std::io::ErrorKind::TimedOut);
    assert!(
        elapsed < Duration::from_secs(3),
        "deadline overshot: {elapsed:?}"
    );
}

/// ATTACK 5c — A timed-out child is killed AND reaped (no zombie).
#[test]
fn timed_out_child_is_reaped() {
    let (manifest, source) = read_plugin("hang");
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
    session.set_timeout(Duration::from_millis(500));
    session.load(&source).unwrap();
    let pid = session.pid();
    assert!(session.invoke(json!({})).is_err());
    // The session is still alive here: reaping must not wait for Drop.
    assert!(
        !pid_exists(pid),
        "timed-out child {pid} still in the process table (zombie?)"
    );
}

/// ATTACK 5d — Dropping a Session (e.g. on an error path) kills and reaps the
/// child, even one whose event loop would otherwise keep it alive forever.
#[test]
fn dropping_session_kills_and_reaps_child() {
    let (manifest, source) = hostile();
    let mut session = Session::spawn(&host_script(), &manifest, focused()).expect("spawn");
    session.load(&source).unwrap();
    let pid = session.pid();
    assert!(pid_exists(pid));
    drop(session);
    assert!(!pid_exists(pid), "dropped session leaked child {pid}");
}

/// Defense in depth for frame integrity: a host that authenticates correctly
/// but LIES in `recheck_result` (models a plugin that subverted host logic in
/// its shared realm) cannot flip the verdict — the core returns its own broker's
/// decision, and refuses a verdict for a capability it never brokered.
#[test]
fn recheck_verdict_comes_from_the_broker_not_the_host() {
    let lying_host =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lying-host.mjs");
    let (manifest, _) = read_plugin("benign");
    let mut session = Session::spawn(&lying_host, &manifest, focused()).expect("spawn");
    session.broker.revoke("open:obsidian://");
    let (allowed, reason) = session.recheck("open:obsidian://vault/Note.md").unwrap();
    assert!(
        !allowed,
        "core trusted the host's recheck verdict over the broker"
    );
    assert_eq!(reason, "grant revoked (SC-3)");
    assert!(
        session.recheck("unbrokered:anything").is_err(),
        "core accepted a recheck verdict the broker never issued"
    );
}

// ---------------------------------------------------------------------------
// Focused-resource disclosure (S1 / TN1 / RT-6 / SC-3). The resource a plugin is
// invoked for is graph-adjacent data: it may reach plugin code ONLY through the
// broker, as `read:focused-resource`, and only while that grant is live. The
// hostile fixture's `resource` mode reports every channel it could learn it
// from: what `run()` was handed directly, what the broker served, and every
// byte the core sent over its stdin since the previous run. The assertions are
// on that returned value (a distinctive marker must be absent), not on any
// host-reported verdict.
// ---------------------------------------------------------------------------

/// Marker present in every field of `private_resource()`; must never appear in
/// anything a plugin without the grant returns.
const RESOURCE_MARKER: &str = "7f3a";

fn private_resource() -> FocusedResource {
    FocusedResource {
        id: "res-7f3a".into(),
        name: "Private-7f3a.md".into(),
        uri: "obsidian://vault/Private-7f3a.md".into(),
    }
}

/// Invoke the hostile fixture's `resource` mode twice and return both results.
/// The second run is the authoritative one for the stdin channel: the plugin's
/// stdin tap is installed when its module evaluates, which can race the first
/// `invoke` frame, but it is always in place before the second.
fn peek_resource_twice(session: &mut Session) -> (serde_json::Value, serde_json::Value) {
    let first = session
        .invoke(json!({ "mode": "resource" }))
        .expect("invoke 1");
    let second = session
        .invoke(json!({ "mode": "resource" }))
        .expect("invoke 2");
    (first, second)
}

/// The stdin tap must have seen this run's `invoke` frame, or the stdin half of
/// the assertion would pass vacuously.
fn assert_tap_saw_invoke(r: &serde_json::Value) {
    let stdin = r["stdin"].as_str().expect("stdin tap string");
    assert!(
        stdin.contains(r#""t":"invoke""#),
        "stdin tap missed the invoke frame (vacuous): {stdin:?}"
    );
}

/// A plugin that did NOT declare `read:focused-resource` learns nothing about the
/// resource: not via `run()`'s arguments, not via the broker, not by reading the
/// pipe.
#[test]
fn undeclared_plugin_never_sees_the_focused_resource() {
    let (mut manifest, source) = hostile();
    manifest.needs = vec!["open:obsidian://".into()]; // some grant, just not this one
    let mut session = Session::spawn(&host_script(), &manifest, private_resource()).expect("spawn");
    session.load(&source).unwrap();
    let (first, second) = peek_resource_twice(&mut session);
    session.shutdown().unwrap();
    eprintln!("[resource:undeclared] {first}\n[resource:undeclared] {second}");

    assert_tap_saw_invoke(&second);
    for r in [&first, &second] {
        assert_eq!(r["requestedAllowed"], false, "broker served it: {r}");
        assert!(
            !r.to_string().contains(RESOURCE_MARKER),
            "FAIL-OPEN: plugin without read:focused-resource learned the resource: {r}"
        );
    }
}

/// Regression guard: a plugin that DID declare `read:focused-resource` still
/// gets the single focused resource, from the broker. It is never pushed into
/// `run()` (the broker is the only channel, so every read is audited and
/// revocable).
#[test]
fn declared_plugin_gets_the_focused_resource_from_the_broker() {
    let (manifest, source) = hostile();
    assert!(manifest.needs.iter().any(|n| n == "read:focused-resource"));
    let mut session = Session::spawn(&host_script(), &manifest, private_resource()).expect("spawn");
    session.load(&source).unwrap();
    let (_, r) = peek_resource_twice(&mut session);
    session.shutdown().unwrap();
    eprintln!("[resource:declared] {r}");

    assert_eq!(r["requestedAllowed"], true, "{r}");
    let got = &r["requested"];
    assert!(got.is_object(), "expected a single resource object: {r}");
    assert_eq!(got["id"], "res-7f3a");
    assert_eq!(got["uri"], "obsidian://vault/Private-7f3a.md");
    assert_eq!(got["name"], "Private-7f3a.md");
    assert!(
        r["pushed"].is_null(),
        "resource was pushed into run() instead of going through the broker: {r}"
    );
}

/// SC-3: once `read:focused-resource` is revoked mid-session, the next
/// invocation no longer delivers the resource through any channel.
#[test]
fn revoked_grant_stops_delivering_the_focused_resource() {
    let (manifest, source) = hostile();
    let mut session = Session::spawn(&host_script(), &manifest, private_resource()).expect("spawn");
    session.load(&source).unwrap();

    let before = session
        .invoke(json!({ "mode": "resource" }))
        .expect("invoke before revoke");
    assert_eq!(before["requested"]["id"], "res-7f3a", "{before}");

    session.broker.revoke("read:focused-resource");
    let after = session
        .invoke(json!({ "mode": "resource" }))
        .expect("invoke after revoke");
    session.shutdown().unwrap();
    eprintln!("[resource:revoked] before={before}\n[resource:revoked] after={after}");

    assert_tap_saw_invoke(&after);
    assert_eq!(after["requestedAllowed"], false, "{after}");
    assert!(
        !after.to_string().contains(RESOURCE_MARKER),
        "FAIL-OPEN: resource still delivered after its grant was revoked: {after}"
    );
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
    let _ = session.invoke(json!({})).unwrap();
    assert!(session.was_allowed("open:obsidian://vault/Note.md"));

    // Revoke the grant, then have the (still-running) plugin re-request it.
    session.broker.revoke("open:obsidian://");
    let (allowed, reason) = session.recheck("open:obsidian://vault/Note.md").unwrap();
    eprintln!("\n[after revoke] allowed={allowed} reason={reason}\n");
    assert!(!allowed, "revoked capability was still allowed");
    assert_eq!(reason, "grant revoked (SC-3)");

    session.shutdown().unwrap();
}
