// Satisfies: RT-5 (binding), S1 (invariant — incl. net containment), T1
// (boundary), TN1 (capability broker), TN6 (isolation proof / evidence E9), S5
// (DoS bounds: overall deadline, bounded queues, IPC line cap, reaped children).
// See .manifold/linking-manifesto-app.md and DECISIONS.md D1-D4.
//
// The sandbox host: spawns the UNTRUSTED plugin in a separate process and owns the
// single IPC channel (stdin/stdout) to it.
//
// Isolation (Solution Option B), layered:
//   1. `node --permission` -> fs, child_process, worker_threads, inspector,
//      process.binding, WASI denied by the Node runtime.
//   2. macOS `sandbox-exec` jail (`SANDBOX_PROFILE`) -> outbound NETWORK, all
//      FILE WRITES (incl. native writes that bypass --permission, e.g.
//      trace_events) and SIGNALS to any other process are denied by the kernel.
//   3. An EMPTY environment (`env_clear`): no inherited secrets and no
//      `NODE_OPTIONS` that could re-grant permissions or preload code. `node` is
//      resolved to an absolute path by the core; the child never searches PATH.
// On non-macOS layer 2 is not yet wired (Linux seccomp/landlock, Windows
// AppContainer are follow-ups) — `Session::net_jailed()` reports the truth.
//
// Frame integrity (DECISIONS D3): the core generates a random per-session NONCE
// and sends it as the FIRST stdin line; the host reads it before any plugin code
// loads and prefixes every frame with it. The reader thread DROPS every stdout
// line without the nonce, so raw writes by plugin code to fd 1 are never
// accepted. The payload of an authenticated frame is still treated as
// plugin-controlled data: every security decision (cap grants, the recheck
// verdict) is derived from the core's own broker, never from host-reported
// fields.
//
// DoS hardening (S5 / DECISIONS D4):
//   - every operation (handshake, load, invoke, recheck) has an OVERALL
//     deadline; a watchdog thread kills the child at that deadline, which also
//     unblocks a pipe WRITE the plugin stalled by not reading stdin;
//   - host frames flow through a BOUNDED queue and lines are capped at 1 MiB;
//   - plugin stderr is piped, forwarded up to a byte cap, then discarded;
//   - a killed child is always reaped, and `Drop` kills + reaps.
//
// Protocol: newline-delimited JSON.
//   Rust -> host:  <nonce>\n (once, first)  {"t":"load","source":...}
//                  {"t":"invoke","resource":...,"probe":...}
//                  {"t":"cap_response","id":N,...} {"t":"recheck","cap":...} {"t":"shutdown"}
//   host -> Rust:  "<nonce> " + one of:
//                  {"t":"ready"} {"t":"cap_request","id":N,"cap":...}
//                  {"t":"result","value":...} {"t":"recheck_result",...} {"t":"log",...}

use crate::broker::{Broker, Decision, FocusedResource, Manifest};
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Largest single IPC line we will buffer from the plugin (S5: bound memory).
const MAX_LINE_BYTES: u64 = 1024 * 1024; // 1 MiB
/// Default OVERALL deadline for one operation (S5: a hung or chatty plugin is
/// killed, not awaited).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Host frames buffered between the reader thread and the session (S5: bounded).
const FRAME_QUEUE_DEPTH: usize = 16;
/// How long `shutdown` lets the host exit cleanly before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
/// Plugin stderr forwarded to the core's stderr per session; the rest is drained
/// and discarded so a noisy plugin can neither flood logs nor block on stderr.
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// Length of the hex frame nonce (256 bits of hex, 244 of them random).
const NONCE_HEX_LEN: usize = 64;
/// Absolute path: never resolved via PATH.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
/// The macOS jail. `allow default` keeps node itself working; the denies close
/// outbound network, every file write (native writes bypass `--permission`) and
/// signals to any process but the plugin itself. NOTE: `(target others)` is NOT
/// enforced by current macOS — deny-all-then-allow-self is (verified by
/// `tests/sandbox.rs::plugin_cannot_signal_the_core`).
pub const SANDBOX_PROFILE: &str = "(version 1)(allow default)(deny network*)(deny file-write*)\
(deny signal)(allow signal (target self))";

/// A self-reported escape attempt by the plugin (for the human-readable demo).
/// The AUTHORITATIVE proof in tests is independent of this.
#[derive(Debug, Clone)]
pub struct EscapeAttempt {
    pub name: String,
    pub outcome: String,
}

pub struct Session {
    child: Arc<Mutex<Child>>,
    pid: u32,
    stdin: ChildStdin,
    rx: Receiver<io::Result<Value>>,
    timeout: Duration,
    net_jailed: bool,
    watchdog: Watchdog,
    pub broker: Broker,
    /// Audit log of every capability decision made this session.
    pub cap_log: Vec<(String, bool, String)>,
}

// ---- spawning ---------------------------------------------------------------

/// Resolve `node` to an absolute, canonical path using the CORE's PATH. The
/// child gets an empty environment, so it could not search PATH itself; and
/// relative PATH entries (e.g. `.`) are ignored so the cwd can't plant a `node`.
fn resolve_node() -> io::Result<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    resolve_node_in(&path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "`node` (>= 22) not found on PATH: the plugin sandbox needs it",
        )
    })
}

fn resolve_node_in(path: &OsStr) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(exe))
        .find(|candidate| is_executable_file(candidate))
        .map(|p| p.canonicalize().unwrap_or(p))
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Is the macOS jail usable? Probed ONCE with the exact profile we will apply
/// (the old named `-n no-network` profile is killed on current macOS, which
/// silently disabled the jail).
fn os_jail_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        cfg!(target_os = "macos")
            && Command::new(SANDBOX_EXEC)
                .arg("-p")
                .arg(SANDBOX_PROFILE)
                .arg("/usr/bin/true")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
    })
}

/// Build the spawn command: `[sandbox-exec -p PROFILE] <abs node> --permission
/// <host>` with an empty environment. Returns (command, os_jailed).
fn build_command(node: &Path, host_script: &Path) -> (Command, bool) {
    let jailed = os_jail_available();
    let mut c = if jailed {
        let mut c = Command::new(SANDBOX_EXEC);
        c.arg("-p").arg(SANDBOX_PROFILE).arg(node);
        c
    } else {
        Command::new(node)
    };
    c.arg("--permission").arg(host_script);
    // No inherited secrets, no NODE_OPTIONS / NODE_* re-grants or preloads.
    c.env_clear();
    #[cfg(windows)]
    if let Some(v) = std::env::var_os("SYSTEMROOT") {
        c.env("SYSTEMROOT", v); // required by node on Windows; not a secret
    }
    // Don't disclose (or give native writers) the core's working directory.
    #[cfg(unix)]
    c.current_dir("/");
    (c, jailed)
}

/// 64 lowercase hex chars from two v4 UUIDs (OS CSPRNG via getrandom).
fn new_nonce() -> String {
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    // The host reads exactly NONCE_HEX_LEN hex chars + '\n' (plugin-host.mjs).
    debug_assert_eq!(nonce.len(), NONCE_HEX_LEN);
    nonce
}

// ---- frame authentication -----------------------------------------------------

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Accept `line` only if it is `<nonce> <json>`; anything else (a frame written
/// by plugin code straight to fd 1, noise, a wrong nonce) is rejected.
fn parse_frame(line: &[u8], nonce: &[u8]) -> Option<Value> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let n = nonce.len();
    if line.len() <= n + 1 || line[n] != b' ' || !ct_eq(&line[..n], nonce) {
        return None;
    }
    serde_json::from_slice(&line[n + 1..]).ok()
}

/// Reader thread: authenticate + parse capped lines and push them onto a
/// BOUNDED channel. Exits on EOF/error or when the session is gone.
fn spawn_reader(stdout: ChildStdout, nonce: Vec<u8>, tx: SyncSender<io::Result<Value>>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut rejected = 0usize;
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
            match parse_frame(&buf, &nonce) {
                Some(v) => {
                    if tx.send(Ok(v)).is_err() {
                        return;
                    }
                }
                None => {
                    rejected += 1;
                    if rejected == 1 {
                        eprintln!(
                            "[sandbox] dropped unauthenticated stdout line(s) from the plugin \
                             (forged or stray frame)"
                        );
                    }
                }
            }
        }
    });
}

/// Forward plugin stderr (capped), then keep draining so the child never blocks.
fn spawn_stderr_drain(stderr: ChildStderr) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut forwarded = 0usize;
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader).take(4096).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if forwarded < MAX_STDERR_BYTES {
                forwarded += line.len();
                let text = String::from_utf8_lossy(&line);
                eprintln!("[plugin stderr] {}", text.trim_end_matches('\n'));
                if forwarded >= MAX_STDERR_BYTES {
                    eprintln!("[plugin stderr] (cap reached; further plugin stderr discarded)");
                }
            }
        }
    });
}

// ---- child lifecycle ------------------------------------------------------------

fn lock(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(|e| e.into_inner())
}

/// Kill (SIGKILL) and reap. Idempotent: std turns `kill` on an already-reaped
/// child into a no-op, so a recycled pid is never signalled.
fn kill_and_reap(child: &Mutex<Child>) {
    let mut c = lock(child);
    let _ = c.kill();
    let _ = c.wait();
}

#[derive(Default)]
struct WatchState {
    deadline: Option<Instant>,
    fired: bool,
    stop: bool,
}

/// Kills the child when an armed deadline passes. Killing closes the child's
/// pipes, which unblocks the session whether it is blocked reading OR writing.
struct Watchdog {
    shared: Arc<(Mutex<WatchState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    fn start(child: Arc<Mutex<Child>>) -> Watchdog {
        let shared = Arc::new((Mutex::new(WatchState::default()), Condvar::new()));
        let theirs = Arc::clone(&shared);
        let thread = std::thread::spawn(move || {
            let (m, cv) = &*theirs;
            let mut st = m.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if st.stop {
                    return;
                }
                match st.deadline {
                    None => st = cv.wait(st).unwrap_or_else(|e| e.into_inner()),
                    Some(d) => {
                        let now = Instant::now();
                        if now >= d {
                            st.deadline = None;
                            st.fired = true;
                            drop(st);
                            kill_and_reap(&child);
                            st = m.lock().unwrap_or_else(|e| e.into_inner());
                        } else {
                            st = cv
                                .wait_timeout(st, d - now)
                                .unwrap_or_else(|e| e.into_inner())
                                .0;
                        }
                    }
                }
            }
        });
        Watchdog {
            shared,
            thread: Some(thread),
        }
    }

    fn update(&self, f: impl FnOnce(&mut WatchState)) {
        let (m, cv) = &*self.shared;
        f(&mut m.lock().unwrap_or_else(|e| e.into_inner()));
        cv.notify_all();
    }

    fn arm(&self, deadline: Instant) {
        self.update(|st| st.deadline = Some(deadline));
    }

    fn disarm(&self) {
        self.update(|st| st.deadline = None);
    }

    fn fired(&self) -> bool {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fired
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.update(|st| st.stop = true);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "plugin exceeded its deadline — killed (S5)",
    )
}

// ---- session ------------------------------------------------------------------

impl Session {
    /// Spawn the plugin host as a separate, deny-by-default (and on macOS,
    /// OS-jailed) process with an empty environment, and complete the
    /// authenticated handshake.
    pub fn spawn(
        host_script: &Path,
        manifest: &Manifest,
        resource: FocusedResource,
    ) -> io::Result<Session> {
        let node = resolve_node()?;
        // Absolute: the child runs with cwd `/`.
        let host_script = host_script.canonicalize()?;
        let (mut cmd, net_jailed) = build_command(&node, &host_script);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let pid = child.id();
        let nonce = new_nonce();

        let (tx, rx) = mpsc::sync_channel::<io::Result<Value>>(FRAME_QUEUE_DEPTH);
        spawn_reader(stdout, nonce.clone().into_bytes(), tx);
        spawn_stderr_drain(stderr);

        let child = Arc::new(Mutex::new(child));
        let watchdog = Watchdog::start(Arc::clone(&child));
        let mut session = Session {
            child,
            pid,
            stdin,
            rx,
            timeout: DEFAULT_TIMEOUT,
            net_jailed,
            watchdog,
            broker: Broker::new(manifest, resource),
            cap_log: Vec::new(),
        };

        // Handshake: the nonce is the FIRST stdin line and is never sent again;
        // the host must answer with an authenticated `ready` before the deadline.
        // (On error `session` drops here, which kills + reaps the child.)
        session.guarded(|s, deadline| {
            s.write_line(&nonce)?;
            let ready = s.recv(deadline)?;
            if ready.get("t").and_then(|v| v.as_str()) != Some("ready") {
                return Err(io::Error::other(format!("no ready handshake: {ready}")));
            }
            Ok(())
        })?;
        Ok(session)
    }

    /// Whether the OS jail (network, file writes, signals) is active for this
    /// session (true on macOS with sandbox-exec). Callers must not claim OS
    /// containment when this is false.
    pub fn net_jailed(&self) -> bool {
        self.net_jailed
    }

    /// OS process id of the sandboxed host (tests use it to prove the child is
    /// killed AND reaped, i.e. not left running or as a zombie).
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Override the OVERALL per-operation deadline (tests use a short one). It
    /// bounds the whole of `load`/`invoke`/`recheck`, not each message.
    pub fn set_timeout(&mut self, d: Duration) {
        self.timeout = d;
    }

    /// Run one protocol operation under an overall deadline enforced by the
    /// watchdog. Any failure poisons the session: the child is killed and
    /// reaped immediately (not left for `Drop`).
    fn guarded<T>(
        &mut self,
        op: impl FnOnce(&mut Session, Instant) -> io::Result<T>,
    ) -> io::Result<T> {
        let deadline = Instant::now() + self.timeout;
        self.watchdog.arm(deadline);
        let res = op(self, deadline);
        self.watchdog.disarm();
        res.map_err(|e| {
            kill_and_reap(&self.child);
            if self.watchdog.fired() || e.kind() == io::ErrorKind::TimedOut {
                timed_out()
            } else {
                e
            }
        })
    }

    /// Next authenticated host frame, or TimedOut once `deadline` has passed —
    /// regardless of how much traffic the plugin keeps sending.
    fn recv(&mut self, deadline: Instant) -> io::Result<Value> {
        let now = Instant::now();
        if now >= deadline {
            return Err(timed_out());
        }
        match self.rx.recv_timeout(deadline - now) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(timed_out()),
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "reader thread gone",
            )),
        }
    }

    /// Blocking write; bounded because the watchdog kills the child (closing
    /// the pipe) if the operation's deadline passes.
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        self.stdin.write_all(&buf)?;
        self.stdin.flush()
    }

    fn send(&mut self, v: &Value) -> io::Result<()> {
        self.write_line(&v.to_string())
    }

    pub fn load(&mut self, source: &str) -> io::Result<()> {
        self.guarded(|s, _| s.send(&json!({ "t": "load", "source": source })))
    }

    /// Invoke the plugin's `run()`, brokering every capability request until it
    /// returns or the overall deadline kills it.
    pub fn invoke(&mut self, resource: &FocusedResource, probe: Value) -> io::Result<Value> {
        self.guarded(|s, deadline| {
            s.send(&json!({ "t": "invoke", "resource": resource, "probe": probe }))?;
            loop {
                let msg = s.recv(deadline)?;
                match msg.get("t").and_then(|v| v.as_str()) {
                    Some("cap_request") => {
                        s.answer_cap(&msg)?;
                    }
                    Some("result") => return Ok(msg.get("value").cloned().unwrap_or(Value::Null)),
                    Some("log") => {
                        eprintln!("[plugin] {}", msg.get("msg").unwrap_or(&Value::Null))
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "unexpected during invoke: {other:?} ({msg})"
                        )))
                    }
                }
            }
        })
    }

    /// SC-3 driver: re-request a capability mid-session. The verdict is the
    /// BROKER's decision for the capability the host brokered during this
    /// recheck — never the allowed/reason fields the (plugin-influenced) host
    /// reports in `recheck_result`.
    pub fn recheck(&mut self, cap: &str) -> io::Result<(bool, String)> {
        self.guarded(|s, deadline| {
            s.send(&json!({ "t": "recheck", "cap": cap }))?;
            let mut verdict: Option<(bool, String)> = None;
            loop {
                let msg = s.recv(deadline)?;
                match msg.get("t").and_then(|v| v.as_str()) {
                    Some("cap_request") => {
                        let (requested, decision) = s.answer_cap(&msg)?;
                        if requested == cap {
                            verdict = Some((decision.allowed, decision.reason));
                        }
                    }
                    Some("recheck_result") => {
                        return verdict.ok_or_else(|| {
                            io::Error::other(
                                "host reported a recheck result without brokering the capability",
                            )
                        })
                    }
                    Some("log") => {
                        eprintln!("[plugin] {}", msg.get("msg").unwrap_or(&Value::Null))
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "unexpected during recheck: {other:?} ({msg})"
                        )))
                    }
                }
            }
        })
    }

    fn answer_cap(&mut self, msg: &Value) -> io::Result<(String, Decision)> {
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
        }))?;
        Ok((cap, decision))
    }

    /// Ask the host to exit; if it has not exited within a short grace period
    /// the watchdog kills it. The child is always reaped (here or in `Drop`).
    pub fn shutdown(mut self) -> io::Result<()> {
        let deadline = Instant::now() + SHUTDOWN_GRACE.min(self.timeout);
        self.watchdog.arm(deadline);
        let _ = self.send(&json!({ "t": "shutdown" }));
        let give_up = deadline + Duration::from_millis(250);
        loop {
            match lock(&self.child).try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {}
            }
            if Instant::now() >= give_up {
                break; // Drop kills + reaps
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    pub fn was_allowed(&self, cap: &str) -> bool {
        self.cap_log.iter().any(|(c, ok, _)| c == cap && *ok)
    }
    pub fn was_denied(&self, cap: &str) -> bool {
        self.cap_log.iter().any(|(c, ok, _)| c == cap && !*ok)
    }
}

impl Drop for Session {
    /// Error paths and early returns must never leak (or zombie) the child.
    fn drop(&mut self) {
        kill_and_reap(&self.child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &[u8] = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn line(prefix: &[u8], json: &str) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.push(b' ');
        v.extend_from_slice(json.as_bytes());
        v.push(b'\n');
        v
    }

    #[test]
    fn frame_with_the_session_nonce_is_accepted() {
        let v = parse_frame(&line(NONCE, r#"{"t":"ready"}"#), NONCE).expect("accepted");
        assert_eq!(v["t"], "ready");
    }

    #[test]
    fn frames_without_the_right_nonce_are_rejected() {
        let forged = br#"{"t":"result","value":{"forged":true}}"#;
        // bare JSON (what plugin code writing to fd 1 produces)
        assert!(parse_frame(&[forged.as_slice(), b"\n"].concat(), NONCE).is_none());
        // wrong nonce of the right shape
        let mut wrong = NONCE.to_vec();
        wrong[0] = b'f';
        assert!(parse_frame(&line(&wrong, r#"{"t":"result"}"#), NONCE).is_none());
        // truncated nonce, nonce without separator, nonce with no payload
        assert!(parse_frame(&line(&NONCE[..63], r#"{"t":"result"}"#), NONCE).is_none());
        assert!(parse_frame(&[NONCE, br#"{"t":"result"}"#].concat(), NONCE).is_none());
        assert!(parse_frame(&[NONCE, b" \n"].concat(), NONCE).is_none());
        // right nonce, malformed JSON
        assert!(parse_frame(&line(NONCE, "{not json"), NONCE).is_none());
    }

    #[test]
    fn nonce_is_64_hex_chars_and_unique() {
        let a = new_nonce();
        let b = new_nonce();
        assert_eq!(a.len(), NONCE_HEX_LEN);
        assert!(a
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn ct_eq_matches_only_identical_slices() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[cfg(unix)]
    #[test]
    fn node_is_resolved_to_an_absolute_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ubilink-resolve-{}", std::process::id()));
        let not_exec = root.join("a");
        let exec = root.join("b");
        std::fs::create_dir_all(&not_exec).unwrap();
        std::fs::create_dir_all(&exec).unwrap();
        std::fs::write(not_exec.join("node"), "").unwrap(); // mode 644: skipped
        std::fs::write(exec.join("node"), "").unwrap();
        std::fs::set_permissions(exec.join("node"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let path = std::env::join_paths([
            PathBuf::from("relative-entry-is-ignored"),
            not_exec.clone(),
            exec.clone(),
        ])
        .unwrap();
        let got = resolve_node_in(&path).expect("resolved");
        assert!(got.is_absolute());
        assert_eq!(got, exec.join("node").canonicalize().unwrap());
        assert!(resolve_node_in(OsStr::new("relative-only")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jail_profile_denies_network_writes_and_foreign_signals() {
        for clause in [
            "(deny network*)",
            "(deny file-write*)",
            "(deny signal)",
            "(allow signal (target self))",
        ] {
            assert!(SANDBOX_PROFILE.contains(clause), "missing {clause}");
        }
    }
}
