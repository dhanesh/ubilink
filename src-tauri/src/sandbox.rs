// Satisfies: RT-5 (binding), S1 (invariant), T1 (boundary), TN1 (capability
// broker), TN6 (this is the isolation proof / manifold evidence E9).
// See .manifold/linking-manifesto-app.md
//
// The sandbox host: spawns the UNTRUSTED plugin in a separate Node process under
// the Node Permission Model and owns the single IPC channel (stdin/stdout) to it.
//
// Isolation mechanism (Solution Option B):
//   node --permission   -> fs, child_process, worker_threads denied by default.
//   (no --allow-fs*, no --allow-child-process, no --allow-worker)
// The plugin SOURCE is streamed in over the pipe (the trusted Rust core read it
// from disk; the host has no fs rights of its own) and loaded via a `data:` URL
// import, which touches no filesystem. So the plugin runs with genuinely zero
// ambient fs/child/worker capability — its only outward channel is a capability
// request over this pipe, which the broker adjudicates.
//
// Protocol: newline-delimited JSON, both directions.
//   Rust -> host:  {"t":"load","source":"<plugin source>"}
//                  {"t":"invoke","resource":{...},"probe":{...}}
//                  {"t":"cap_response","id":N,"allowed":bool,"result":...,"reason":...}
//                  {"t":"recheck","cap":"..."}        (re-request a cap mid-session, for SC-3)
//                  {"t":"shutdown"}
//   host -> Rust:  {"t":"ready"}
//                  {"t":"cap_request","id":N,"cap":"..."}
//                  {"t":"result","value":...}         (plugin run() returned)
//                  {"t":"recheck_result","cap":"...","allowed":bool,"reason":"..."}
//                  {"t":"log","msg":...}

use crate::broker::{Broker, FocusedResource, Manifest};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A self-reported escape attempt by the plugin (for the human-readable
/// demonstration). NOTE: the AUTHORITATIVE proof in the tests is independent —
/// the test checks sentinel files on disk and scans output — not the plugin's
/// own claim (a malicious plugin could lie about its own outcome).
#[derive(Debug, Clone)]
pub struct EscapeAttempt {
    pub name: String,
    pub outcome: String,
}

pub struct Session {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    pub broker: Broker,
    /// Audit log of every capability decision made this session.
    pub cap_log: Vec<(String, bool, String)>,
}

impl Session {
    /// Spawn the plugin host as a separate, deny-by-default Node process.
    pub fn spawn(host_script: &Path, manifest: &Manifest, resource: FocusedResource) -> io::Result<Session> {
        let mut child = Command::new("node")
            // The whole point: deny fs/child_process/worker by default. We pass
            // NONE of --allow-fs-read / --allow-fs-write / --allow-child-process /
            // --allow-worker, so the plugin host (and the plugin inside it) cannot
            // touch the filesystem, spawn processes, or start worker threads.
            .arg("--permission")
            .arg(host_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // surface host/plugin diagnostics in test output
            .spawn()?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = BufReader::new(stdout);

        // Wait for the host's ready handshake.
        let ready = read_msg(&mut reader)?;
        if ready.get("t").and_then(|v| v.as_str()) != Some("ready") {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("host did not send ready handshake, got: {ready}"),
            ));
        }

        Ok(Session {
            child,
            stdin,
            reader,
            broker: Broker::new(manifest, resource),
            cap_log: Vec::new(),
        })
    }

    fn send(&mut self, v: &Value) -> io::Result<()> {
        self.stdin.write_all(v.to_string().as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    /// Stream the (trusted-core-read) plugin source into the sandbox.
    pub fn load(&mut self, source: &str) -> io::Result<()> {
        self.send(&json!({ "t": "load", "source": source }))
    }

    /// Invoke the plugin's `run()` with the focused resource and an optional
    /// `probe` object (used by the malicious example to know where to attempt
    /// escapes). Pumps the protocol, brokering every capability request, until
    /// the plugin returns. Returns the plugin's own returned value.
    pub fn invoke(&mut self, resource: &FocusedResource, probe: Value) -> io::Result<Value> {
        self.send(&json!({ "t": "invoke", "resource": resource, "probe": probe }))?;
        loop {
            let msg = read_msg(&mut self.reader)?;
            match msg.get("t").and_then(|v| v.as_str()) {
                Some("cap_request") => self.answer_cap(&msg)?,
                Some("result") => return Ok(msg.get("value").cloned().unwrap_or(Value::Null)),
                Some("log") => eprintln!("[plugin] {}", msg.get("msg").unwrap_or(&Value::Null)),
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("unexpected message during invoke: {other:?} ({msg})"),
                    ))
                }
            }
        }
    }

    /// SC-3 driver: ask the plugin to re-request a capability (simulating a
    /// long-lived plugin doing periodic work) and report whether it is now
    /// allowed. Combined with `self.broker.revoke(..)`, this proves a revoked
    /// grant takes effect on the next request.
    pub fn recheck(&mut self, cap: &str) -> io::Result<(bool, String)> {
        self.send(&json!({ "t": "recheck", "cap": cap }))?;
        loop {
            let msg = read_msg(&mut self.reader)?;
            match msg.get("t").and_then(|v| v.as_str()) {
                Some("cap_request") => self.answer_cap(&msg)?,
                Some("recheck_result") => {
                    let allowed = msg.get("allowed").and_then(|v| v.as_bool()).unwrap_or(false);
                    let reason = msg
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    return Ok((allowed, reason));
                }
                Some("log") => eprintln!("[plugin] {}", msg.get("msg").unwrap_or(&Value::Null)),
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("unexpected message during recheck: {other:?} ({msg})"),
                    ))
                }
            }
        }
    }

    fn answer_cap(&mut self, msg: &Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let cap = msg.get("cap").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let decision = self.broker.decide(&cap);
        self.cap_log
            .push((cap.clone(), decision.allowed, decision.reason.clone()));
        self.send(&json!({
            "t": "cap_response",
            "id": id,
            "allowed": decision.allowed,
            "result": decision.result,
            "reason": decision.reason,
        }))
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        let _ = self.send(&json!({ "t": "shutdown" }));
        let _ = self.child.wait();
        Ok(())
    }

    /// Whether a given capability was decided ALLOWED at least once this session.
    pub fn was_allowed(&self, cap: &str) -> bool {
        self.cap_log.iter().any(|(c, ok, _)| c == cap && *ok)
    }
    /// Whether a given capability was decided DENIED at least once this session.
    pub fn was_denied(&self, cap: &str) -> bool {
        self.cap_log.iter().any(|(c, ok, _)| c == cap && !*ok)
    }
}

fn read_msg(reader: &mut BufReader<ChildStdout>) -> io::Result<Value> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "plugin host closed the pipe unexpectedly",
        ));
    }
    serde_json::from_str(line.trim_end())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}: {line:?}")))
}
