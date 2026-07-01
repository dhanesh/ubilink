// Satisfies: RT-5 (binding), S1 (invariant — NOW incl. net containment), T1
// (boundary), TN1 (capability broker), TN6 (isolation proof / evidence E9), S5
// (DoS bounds: exec timeout + IPC line cap).
// See .manifold/linking-manifesto-app.md
//
// The sandbox host: spawns the UNTRUSTED plugin in a separate process and owns the
// single IPC channel (stdin/stdout) to it.
//
// Isolation mechanism (Solution Option B), two OS-level layers:
//   1. `node --permission` -> fs, child_process, worker_threads denied by default.
//   2. macOS `sandbox-exec` deny-network profile around the host -> OUTBOUND
//      NETWORK denied (closes the gap that `--permission` alone left open; see
//      DECISIONS D2). Proven by `tests/sandbox.rs::net_is_jailed_*`.
// On non-macOS the network jail is not yet wired (Linux seccomp / Windows
// AppContainer are the follow-ups) — `Session::net_jailed()` reports the truth so
// callers and tests never over-claim.
//
// DoS hardening (S5 / DECISIONS D4): host reads run on a reader thread with a
// per-message TIMEOUT (a hung plugin is killed, not waited on forever) and a
// LINE-SIZE CAP (an unbounded line cannot exhaust memory).
//
// Protocol: newline-delimited JSON, both directions (unchanged).
//   Rust -> host:  {"t":"load","source":...} {"t":"invoke","resource":...,"probe":...}
//                  {"t":"cap_response","id":N,...} {"t":"recheck","cap":...} {"t":"shutdown"}
//   host -> Rust:  {"t":"ready"} {"t":"cap_request","id":N,"cap":...}
//                  {"t":"result","value":...} {"t":"recheck_result",...} {"t":"log",...}

use crate::broker::{Broker, FocusedResource, Manifest};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// Largest single IPC line we will buffer from the plugin (S5: bound memory).
const MAX_LINE_BYTES: u64 = 1024 * 1024; // 1 MiB
/// Default per-message read timeout (S5: a hung plugin is killed, not awaited).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A self-reported escape attempt by the plugin (for the human-readable demo).
/// The AUTHORITATIVE proof in tests is independent of this.
#[derive(Debug, Clone)]
pub struct EscapeAttempt {
    pub name: String,
    pub outcome: String,
}

pub struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<io::Result<Value>>,
    timeout: Duration,
    net_jailed: bool,
    pub broker: Broker,
    /// Audit log of every capability decision made this session.
    pub cap_log: Vec<(String, bool, String)>,
}

/// Build the spawn command. On macOS (when `sandbox-exec` is available) the host
/// is wrapped in a deny-network jail; everywhere else it is a plain
/// `node --permission` invocation. Returns (command, net_jailed).
fn build_command(host_script: &Path) -> (Command, bool) {
    let node_args = ["--permission"];
    if cfg!(target_os = "macos") && which_sandbox_exec() {
        // (allow default) then (deny network*) => everything allowed EXCEPT network.
        let profile = "(version 1)(allow default)(deny network*)";
        let mut c = Command::new("sandbox-exec");
        c.arg("-p")
            .arg(profile)
            .arg("node")
            .args(node_args)
            .arg(host_script);
        (c, true)
    } else {
        let mut c = Command::new("node");
        c.args(node_args).arg(host_script);
        (c, false)
    }
}

fn which_sandbox_exec() -> bool {
    Command::new("sandbox-exec")
        .arg("-n")
        .arg("no-network")
        .arg("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl Session {
    /// Spawn the plugin host as a separate, deny-by-default (and on macOS,
    /// net-jailed) process.
    pub fn spawn(
        host_script: &Path,
        manifest: &Manifest,
        resource: FocusedResource,
    ) -> io::Result<Session> {
        let (mut cmd, net_jailed) = build_command(host_script);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        // Reader thread: parse newline-delimited JSON (capped) and push onto a
        // channel so the main thread can read with a timeout (S5).
        let (tx, rx) = mpsc::channel::<io::Result<Value>>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut buf = Vec::new();
                // `take` bounds memory at MAX_LINE_BYTES regardless of plugin output.
                let n = match (&mut reader)
                    .take(MAX_LINE_BYTES)
                    .read_until(b'\n', &mut buf)
                {
                    Ok(n) => n,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                if n == 0 {
                    let _ = tx.send(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "plugin host closed the pipe",
                    )));
                    return;
                }
                if !buf.ends_with(b"\n") && buf.len() as u64 >= MAX_LINE_BYTES {
                    let _ = tx.send(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "IPC line exceeded size cap (S5)",
                    )));
                    return;
                }
                let line = String::from_utf8_lossy(&buf);
                match serde_json::from_str::<Value>(line.trim_end()) {
                    Ok(v) => {
                        if tx.send(Ok(v)).is_err() {
                            return;
                        }
                    }
                    Err(_) => { /* skip malformed line, keep reading */ }
                }
            }
        });

        let mut session = Session {
            child,
            stdin,
            rx,
            timeout: DEFAULT_TIMEOUT,
            net_jailed,
            broker: Broker::new(manifest, resource),
            cap_log: Vec::new(),
        };

        // Wait (with timeout) for the host's ready handshake.
        let ready = session.recv()?;
        if ready.get("t").and_then(|v| v.as_str()) != Some("ready") {
            return Err(io::Error::other(format!("no ready handshake: {ready}")));
        }
        Ok(session)
    }

    /// Whether outbound network is OS-jailed for this session (true on macOS with
    /// sandbox-exec). Callers must not claim net containment when this is false.
    pub fn net_jailed(&self) -> bool {
        self.net_jailed
    }

    /// Override the per-message read timeout (tests use a short one).
    pub fn set_timeout(&mut self, d: Duration) {
        self.timeout = d;
    }

    /// Read the next host message, killing the child if it does not arrive within
    /// the timeout (S5: no unbounded wait on a hung plugin).
    fn recv(&mut self) -> io::Result<Value> {
        match self.rx.recv_timeout(self.timeout) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "plugin exceeded execution timeout — killed (S5)",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "reader thread gone",
            )),
        }
    }

    fn send(&mut self, v: &Value) -> io::Result<()> {
        self.stdin.write_all(v.to_string().as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    pub fn load(&mut self, source: &str) -> io::Result<()> {
        self.send(&json!({ "t": "load", "source": source }))
    }

    /// Invoke the plugin's `run()`, brokering every capability request until it
    /// returns (or the timeout kills it).
    pub fn invoke(&mut self, resource: &FocusedResource, probe: Value) -> io::Result<Value> {
        self.send(&json!({ "t": "invoke", "resource": resource, "probe": probe }))?;
        loop {
            let msg = self.recv()?;
            match msg.get("t").and_then(|v| v.as_str()) {
                Some("cap_request") => self.answer_cap(&msg)?,
                Some("result") => return Ok(msg.get("value").cloned().unwrap_or(Value::Null)),
                Some("log") => eprintln!("[plugin] {}", msg.get("msg").unwrap_or(&Value::Null)),
                other => {
                    return Err(io::Error::other(format!(
                        "unexpected during invoke: {other:?} ({msg})"
                    )))
                }
            }
        }
    }

    /// SC-3 driver: re-request a capability mid-session.
    pub fn recheck(&mut self, cap: &str) -> io::Result<(bool, String)> {
        self.send(&json!({ "t": "recheck", "cap": cap }))?;
        loop {
            let msg = self.recv()?;
            match msg.get("t").and_then(|v| v.as_str()) {
                Some("cap_request") => self.answer_cap(&msg)?,
                Some("recheck_result") => {
                    let allowed = msg
                        .get("allowed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let reason = msg
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    return Ok((allowed, reason));
                }
                Some("log") => eprintln!("[plugin] {}", msg.get("msg").unwrap_or(&Value::Null)),
                other => {
                    return Err(io::Error::other(format!(
                        "unexpected during recheck: {other:?} ({msg})"
                    )))
                }
            }
        }
    }

    fn answer_cap(&mut self, msg: &Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let cap = msg
            .get("cap")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let decision = self.broker.decide(&cap);
        self.cap_log
            .push((cap.clone(), decision.allowed, decision.reason.clone()));
        self.send(&json!({
            "t": "cap_response", "id": id,
            "allowed": decision.allowed, "result": decision.result, "reason": decision.reason,
        }))
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        let _ = self.send(&json!({ "t": "shutdown" }));
        let _ = self.child.wait();
        Ok(())
    }

    pub fn was_allowed(&self, cap: &str) -> bool {
        self.cap_log.iter().any(|(c, ok, _)| c == cap && *ok)
    }
    pub fn was_denied(&self, cap: &str) -> bool {
        self.cap_log.iter().any(|(c, ok, _)| c == cap && !*ok)
    }
}
