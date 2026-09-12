// Satisfies: O4 (a registry where signed integrations are published, discovered and
// fetched — the ecosystem distribution point B3's extension surface feeds into).
// See .manifold/linking-manifesto-app.md
//
// Functional-core / imperative-shell, same as the rest of ubilink: `RegistryIndex`
// (publish/get/list/search) and the pure `handle_request` router are 100% testable
// with no sockets; `bin/ubilink-registry.rs` is the thin std-only TcpListener shell.
//
// Trust reuses ubilink-core's proven model: a publish is accepted ONLY if the bundle
// verifies (ed25519 + manifest schema, via ubilink_core::verify_json) against the
// registry's trusted publisher set, AND the id is either new or owned by the same
// signing key (first publisher wins the id — being trusted for SOME id is not
// being trusted for EVERY id), AND its version is strictly newer than what's
// already published (monotonicity — no silent downgrade).
//
// Key rotation / id transfer is NOT supported yet: an id stays bound to the key
// that first published it for the life of the index.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What the registry stores for a published integration id.
struct Entry {
    wire_json: String, // the exact signed bundle, served back verbatim
    version: u32,
    name: String,
    app: String,
    /// Lowercase hex of the ed25519 key that first published this id. Stored with
    /// the entry so any future persistence of the index persists ownership too.
    owner_hex: String,
}

/// A row in a listing / search result (no source/signature — just discovery data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub app: String,
}

impl ListItem {
    fn to_json(&self) -> String {
        format!(
            r#"{{"id":{},"version":{},"name":{},"app":{}}}"#,
            json_str(&self.id),
            self.version,
            json_str(&self.name),
            json_str(&self.app)
        )
    }
}

/// The registry's in-memory index. (Persistence to disk is a deployment concern —
/// see docs/RUNBOOK-registry.md; the logic here is storage-agnostic.)
pub struct RegistryIndex {
    trusted_pub_hex: Vec<String>,
    by_id: HashMap<String, Entry>,
}

impl RegistryIndex {
    pub fn new(trusted_pub_hex: Vec<String>) -> RegistryIndex {
        RegistryIndex {
            trusted_pub_hex,
            by_id: HashMap::new(),
        }
    }

    /// Publish a signed bundle (the JSON `ubilink-sign sign` produces). Accepted iff it
    /// verifies against a trusted publisher with a schema-valid manifest, the id is
    /// new or owned by the same signer, AND it is strictly newer than any existing
    /// version for the same id. Reuses ubilink-core's crypto verbatim (no reimpl).
    pub fn publish(&mut self, wire_json: &str) -> Result<(), String> {
        // 1) crypto + trusted-publisher + manifest-schema check (the R3/R8 proven
        //    path; `Installer::verify` runs `Manifest::validate`, and serde refuses
        //    unknown manifest fields).
        ubilink_core::verify_json(wire_json, &self.trusted_pub_hex).map_err(|e| e.message())?;
        // 2) parse for id/version/name/app (WireBundle is the public wire format).
        let wire: ubilink_core::WireBundle =
            serde_json::from_str(wire_json).map_err(|e| format!("malformed bundle JSON: {e}"))?;
        // verify_json accepted signer_hex as 64 hex chars (surrounding whitespace
        // tolerated, either case) — normalise so ownership compares key bytes.
        let signer_hex = wire.signer_hex.trim().to_ascii_lowercase();
        if let Some(cur) = self.by_id.get(&wire.manifest.id) {
            // 3) ownership: only the key that first published an id may update it.
            if cur.owner_hex != signer_hex {
                return Err(format!(
                    "integration id {:?} is owned by publisher key {}; a bundle signed by \
                     {} cannot replace it (ids are bound to their first publisher; key \
                     rotation is not supported yet)",
                    wire.manifest.id, cur.owner_hex, signer_hex
                ));
            }
            // 4) monotonicity: never replace a published version with an older/equal one.
            if wire.version <= cur.version {
                return Err(format!(
                    "version {} is not newer than the published version {}",
                    wire.version, cur.version
                ));
            }
        }
        self.by_id.insert(
            wire.manifest.id.clone(),
            Entry {
                wire_json: wire_json.to_string(),
                version: wire.version,
                name: wire.manifest.name.clone(),
                app: wire.manifest.app.clone(),
                owner_hex: signer_hex,
            },
        );
        Ok(())
    }

    /// The publisher key (lowercase hex) that owns an id, i.e. its first signer.
    pub fn owner(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(|e| e.owner_hex.as_str())
    }

    /// The signed bundle JSON for an id, served back verbatim for install.
    pub fn get(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(|e| e.wire_json.as_str())
    }

    /// All published integrations, sorted by id for a stable listing.
    pub fn list(&self) -> Vec<ListItem> {
        let mut items: Vec<ListItem> = self
            .by_id
            .iter()
            .map(|(id, e)| ListItem {
                id: id.clone(),
                version: e.version,
                name: e.name.clone(),
                app: e.app.clone(),
            })
            .collect();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        items
    }

    /// Case-insensitive substring search over id / name / app (discovery).
    pub fn search(&self, query: &str) -> Vec<ListItem> {
        let q = query.to_lowercase();
        self.list()
            .into_iter()
            .filter(|it| {
                it.id.to_lowercase().contains(&q)
                    || it.name.to_lowercase().contains(&q)
                    || it.app.to_lowercase().contains(&q)
            })
            .collect()
    }
}

/// A handled HTTP response (status + JSON body). Returned by the pure router so the
/// socket shell only has to format bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resp {
    pub status: u16,
    pub body: String,
}

impl Resp {
    fn ok(body: String) -> Resp {
        Resp { status: 200, body }
    }
    fn created(body: String) -> Resp {
        Resp { status: 201, body }
    }
    fn bad(msg: &str) -> Resp {
        Resp {
            status: 400,
            body: error_json(msg),
        }
    }
    fn not_found(msg: &str) -> Resp {
        Resp::error(404, msg)
    }
    fn error(status: u16, msg: &str) -> Resp {
        Resp {
            status,
            body: error_json(msg),
        }
    }
}

/// Pure request router (no I/O). The whole API surface, testable without sockets.
///   GET  /integrations          -> list
///   GET  /integrations?q=term   -> search
///   GET  /integrations/{id}     -> the signed bundle (verbatim) or 404
///   POST /publish  (body=bundle)-> publish, 201 / 400
pub fn handle_request(method: &str, path: &str, body: &str, idx: &mut RegistryIndex) -> Resp {
    match method {
        "GET" => {
            let (route, query) = path.split_once('?').unwrap_or((path, ""));
            if route == "/integrations" {
                let items = match query_param(query, "q") {
                    Some(q) => idx.search(&q),
                    None => idx.list(),
                };
                let arr: Vec<String> = items.iter().map(ListItem::to_json).collect();
                Resp::ok(format!("[{}]", arr.join(",")))
            } else if let Some(id) = route.strip_prefix("/integrations/") {
                match idx.get(&percent_decode(id)) {
                    Some(json) => Resp::ok(json.to_string()),
                    None => Resp::not_found("no such integration"),
                }
            } else {
                Resp::not_found("unknown route")
            }
        }
        "POST" if path == "/publish" => match idx.publish(body) {
            Ok(()) => Resp::created(r#"{"published":true}"#.to_string()),
            Err(e) => Resp::bad(&e),
        },
        _ => Resp::not_found("unknown route"),
    }
}

/// Per-request size caps, checked BEFORE anything is allocated for the request.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Request line incl. CRLF. 8 KiB matches common server defaults; our longest
    /// legitimate line is `GET /integrations/<id>` or a `?q=` search term.
    pub max_request_line: usize,
    /// All header lines together (excluding the request line).
    pub max_header_bytes: usize,
    /// Body (`Content-Length`) cap. A bundle is a small manifest, one plugin source
    /// and a hex signature/key: the repo's example plugins are 0.3-7 KiB, so their
    /// bundles are a few KiB. 1 MiB is >100x that headroom (even with worst-case
    /// JSON escaping of the source) while bounding per-connection memory, and with
    /// `max_connections` bounds total body buffering to 64 MiB.
    pub max_body: usize,
}

pub const DEFAULT_MAX_REQUEST_LINE: usize = 8 * 1024;
pub const DEFAULT_MAX_HEADER_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_BODY: usize = 1024 * 1024;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_request_line: DEFAULT_MAX_REQUEST_LINE,
            max_header_bytes: DEFAULT_MAX_HEADER_BYTES,
            max_body: DEFAULT_MAX_BODY,
        }
    }
}

/// Resource bounds for the socket shell (`serve`).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub limits: Limits,
    /// Concurrent connections (= handler threads). Over the limit, a connection is
    /// answered `503` from the accept loop and closed. 64 is ample for a
    /// low-traffic registry and caps threads + buffered bodies (64 x 1 MiB).
    pub max_connections: usize,
    /// Max time a single socket read or write may wait for progress.
    pub idle_timeout: Duration,
    /// Overall budget for reading a whole request (and, separately, for writing
    /// the response), so a client dribbling one byte per `idle_timeout` still
    /// cannot hold a slot indefinitely. 30 s reads a 1 MiB body at ~35 KB/s.
    pub request_deadline: Duration,
}

impl Default for ServerConfig {
    fn default() -> ServerConfig {
        ServerConfig {
            limits: Limits::default(),
            max_connections: 64,
            idle_timeout: Duration::from_secs(10),
            request_deadline: Duration::from_secs(30),
        }
    }
}

/// The accept loop: one bounded thread per connection, each with socket timeouts
/// and a request deadline; over `max_connections`, answer 503. Std-only.
pub fn serve(
    listener: TcpListener,
    idx: Arc<Mutex<RegistryIndex>>,
    cfg: ServerConfig,
) -> io::Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    let cfg = Arc::new(cfg);
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                // e.g. EMFILE: back off briefly instead of spinning on the error.
                eprintln!("accept error: {e}");
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        // Only this thread increments, so check-then-increment cannot overshoot.
        if active.load(Ordering::Acquire) >= cfg.max_connections {
            reject_busy(stream);
            continue;
        }
        active.fetch_add(1, Ordering::AcqRel);
        let slot = Slot(Arc::clone(&active));
        let idx = Arc::clone(&idx);
        let cfg = Arc::clone(&cfg);
        let spawned = std::thread::Builder::new()
            .name("registry-conn".into())
            .spawn(move || {
                let _slot = slot; // released on return AND on panic
                if let Err(e) = serve_connection(stream, &idx, &cfg) {
                    eprintln!("connection error: {e}");
                }
            });
        if let Err(e) = spawned {
            // The closure (and its Slot) was dropped, so the slot is released.
            eprintln!("could not spawn connection thread: {e}");
        }
    }
    Ok(())
}

/// A held connection slot; dropping it frees the slot.
struct Slot(Arc<AtomicUsize>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answer 503 without blocking the accept loop: non-blocking best-effort drain of
/// whatever the client already sent (so closing does not reset the reply away),
/// a tiny response that fits the socket buffer, then close.
fn reject_busy(mut stream: TcpStream) {
    let _ = stream.set_nonblocking(true);
    let mut sink = [0u8; 4096];
    let _ = stream.read(&mut sink);
    let _ = write_response(
        &mut stream,
        &Resp::error(503, "registry busy: too many concurrent connections, retry"),
    );
    let _ = stream.shutdown(Shutdown::Write);
}

fn serve_connection(
    stream: TcpStream,
    idx: &Mutex<RegistryIndex>,
    cfg: &ServerConfig,
) -> io::Result<()> {
    let mut conn = DeadlineStream {
        inner: stream,
        deadline: Instant::now() + cfg.request_deadline,
        budget: cfg.request_deadline,
        idle: cfg.idle_timeout,
        writing: false,
    };
    handle_stream(&mut conn, idx, &cfg.limits)
}

/// A TcpStream whose every read/write waits at most `idle`, and never past
/// `deadline`. The deadline restarts once when the response starts being written.
struct DeadlineStream {
    inner: TcpStream,
    deadline: Instant,
    budget: Duration,
    idle: Duration,
    writing: bool,
}

impl DeadlineStream {
    fn wait_budget(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "request deadline exceeded",
            ));
        }
        Ok(left.min(self.idle))
    }
}

/// std reports an expired socket timeout as WouldBlock on Unix; name it.
fn as_timeout(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::WouldBlock {
        io::Error::new(io::ErrorKind::TimedOut, "socket idle timeout")
    } else {
        e
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let wait = self.wait_budget()?;
        self.inner.set_read_timeout(Some(wait))?;
        self.inner.read(buf).map_err(as_timeout)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.writing {
            self.writing = true;
            self.deadline = Instant::now() + self.budget;
        }
        let wait = self.wait_budget()?;
        self.inner.set_write_timeout(Some(wait))?;
        self.inner.write(buf).map_err(as_timeout)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A fully-read request, ready to route.
struct Request {
    method: String,
    path: String,
    body: String,
}

/// What reading a request produced: a request, an early error reply, or nothing
/// (the peer closed before sending a byte).
enum ReadOutcome {
    Request(Request),
    Reject(Resp),
    Closed,
}

enum Line {
    Complete(Vec<u8>),
    TooLong,
    Truncated,
    Eof,
}

/// Read one `\n`-terminated line, consuming at most `cap + 1` bytes.
fn read_line_capped<R: BufRead>(r: &mut R, cap: usize) -> io::Result<Line> {
    let mut buf = Vec::new();
    r.take(cap as u64 + 1).read_until(b'\n', &mut buf)?;
    Ok(if buf.is_empty() {
        Line::Eof
    } else if buf.len() > cap {
        Line::TooLong
    } else if buf.ends_with(b"\n") {
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        Line::Complete(buf)
    } else {
        Line::Truncated
    })
}

/// Parse one HTTP/1.1 request with every size bounded. Holds no lock — callers
/// must not either (a slow client would otherwise stall every other request).
fn read_request<R: Read>(stream: R, limits: &Limits) -> io::Result<ReadOutcome> {
    let mut reader = BufReader::new(stream);
    let bad = |msg: &str| Ok(ReadOutcome::Reject(Resp::bad(msg)));

    // request line
    let line = match read_line_capped(&mut reader, limits.max_request_line)? {
        Line::Eof => return Ok(ReadOutcome::Closed),
        Line::TooLong => {
            return Ok(ReadOutcome::Reject(Resp::error(
                414,
                "request line too long",
            )))
        }
        Line::Truncated => return bad("incomplete request line"),
        Line::Complete(l) => l,
    };
    let Ok(line) = String::from_utf8(line) else {
        return bad("request line is not valid UTF-8");
    };
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // headers — we act on Content-Length and refuse Transfer-Encoding.
    let too_big = || {
        Ok(ReadOutcome::Reject(Resp::error(
            431,
            "request header section too large",
        )))
    };
    let mut header_budget = limits.max_header_bytes;
    let mut content_length: Option<u64> = None;
    loop {
        let line = match read_line_capped(&mut reader, header_budget)? {
            Line::TooLong => return too_big(),
            Line::Eof | Line::Truncated => return bad("incomplete header section"),
            Line::Complete(l) => l,
        };
        // +2: the CRLF read_line_capped stripped (conservative for bare LF).
        header_budget = match header_budget.checked_sub(line.len() + 2) {
            Some(left) => left,
            None => return too_big(),
        };
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = std::str::from_utf8(&line)
            .ok()
            .and_then(|l| l.split_once(':'))
        else {
            return bad("malformed header line");
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return bad("duplicate Content-Length");
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return bad("invalid Content-Length");
            }
            // All digits: the only parse failure left is u64 overflow — too large.
            content_length = Some(value.parse().unwrap_or(u64::MAX));
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Ok(ReadOutcome::Reject(Resp::error(
                501,
                "Transfer-Encoding is not supported; send a Content-Length body",
            )));
        }
    }

    // body — the cap is checked BEFORE allocating, and exactly Content-Length
    // bytes are read (nothing past it is ever buffered).
    if method == "POST" && content_length.is_none() {
        return Ok(ReadOutcome::Reject(Resp::error(
            411,
            "POST requires a Content-Length",
        )));
    }
    let len = content_length.unwrap_or(0);
    if len > limits.max_body as u64 {
        return Ok(ReadOutcome::Reject(Resp::error(
            413,
            &format!(
                "request body of {len} bytes exceeds the {}-byte limit",
                limits.max_body
            ),
        )));
    }
    let mut body = Vec::with_capacity(len as usize);
    reader.take(len).read_to_end(&mut body)?;
    if (body.len() as u64) < len {
        return bad("request body shorter than Content-Length");
    }
    let Ok(body) = String::from_utf8(body) else {
        return bad("request body is not valid UTF-8");
    };
    Ok(ReadOutcome::Request(Request { method, path, body }))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Content Too Large",
        414 => "URI Too Long",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn write_response<W: Write>(w: &mut W, resp: &Resp) -> io::Result<()> {
    let out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        resp.status,
        reason(resp.status),
        resp.body.len(),
        resp.body
    );
    w.write_all(out.as_bytes())?;
    w.flush()
}

/// Serve ONE HTTP/1.1 request on a stream: read and parse it with every size
/// bounded and NO lock held, take the index lock only to apply the parsed
/// operation, release it, then write the response. Generic over Read+Write so it
/// is unit-tested with an in-memory stream; `serve` passes a deadline-bounded
/// `TcpStream`.
pub fn handle_stream<S: Read + Write>(
    stream: &mut S,
    idx: &Mutex<RegistryIndex>,
    limits: &Limits,
) -> io::Result<()> {
    let resp = match read_request(&mut *stream, limits)? {
        ReadOutcome::Closed => return Ok(()),
        ReadOutcome::Reject(resp) => resp,
        ReadOutcome::Request(req) => {
            // A poisoned lock still serves safely: publish mutates only after
            // every check has passed, in a single insert.
            let mut guard = idx.lock().unwrap_or_else(PoisonError::into_inner);
            handle_request(&req.method, &req.path, &req.body, &mut guard)
        } // lock released here, before any socket write
    };
    write_response(stream, &resp)
}

// ── tiny dependency-free helpers ──
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn error_json(msg: &str) -> String {
    format!(r#"{{"error":{}}}"#, json_str(msg))
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

/// Minimal percent-decoding for path/query segments (enough for ids + search terms).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use ubilink_core::broker::Manifest;
    use ubilink_core::release::{public_hex, sign_to_json, signing_key_from_hex};

    fn key_hex(seed: u8) -> String {
        (0..32).map(|_| format!("{seed:02x}")).collect()
    }
    fn manifest(id: &str, app: &str, name: &str) -> Manifest {
        Manifest {
            id: id.into(),
            name: name.into(),
            app: app.into(),
            version: "1.0.0".into(),
            needs: vec![],
            signature: None,
        }
    }
    fn signed_manifest(seed: u8, m: Manifest, version: u32) -> String {
        let key = signing_key_from_hex(&key_hex(seed)).unwrap();
        sign_to_json(&key, m, "export function run(){}".into(), version).unwrap()
    }
    fn index_trusting_all(seeds: &[u8]) -> RegistryIndex {
        RegistryIndex::new(
            seeds
                .iter()
                .map(|s| public_hex(&signing_key_from_hex(&key_hex(*s)).unwrap()))
                .collect(),
        )
    }
    fn signed(seed: u8, id: &str, app: &str, name: &str, version: u32) -> String {
        let key = signing_key_from_hex(&key_hex(seed)).unwrap();
        sign_to_json(
            &key,
            manifest(id, app, name),
            "export function run(){}".into(),
            version,
        )
        .unwrap()
    }
    fn index_trusting(seed: u8) -> RegistryIndex {
        let pubk = public_hex(&signing_key_from_hex(&key_hex(seed)).unwrap());
        RegistryIndex::new(vec![pubk])
    }

    #[test]
    fn publish_then_get_and_list() {
        let mut idx = index_trusting(7);
        idx.publish(&signed(
            7,
            "com.x.obsidian",
            "Obsidian",
            "Obsidian Linker",
            1,
        ))
        .unwrap();
        assert!(idx.get("com.x.obsidian").is_some());
        let list = idx.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].app, "Obsidian");
        assert_eq!(list[0].version, 1);
    }

    #[test]
    fn publish_rejects_untrusted_publisher() {
        // O4 trust: a bundle signed by a key the registry doesn't trust is refused.
        let mut idx = index_trusting(7);
        let by_attacker = signed(9, "com.evil.app", "Evil", "Evil", 1);
        assert!(idx.publish(&by_attacker).is_err());
        assert!(idx.get("com.evil.app").is_none());
    }

    #[test]
    fn publish_enforces_version_monotonicity() {
        let mut idx = index_trusting(7);
        idx.publish(&signed(7, "com.x.app", "App", "App", 2))
            .unwrap();
        // re-publishing an older/equal version is refused (no silent downgrade).
        assert!(idx
            .publish(&signed(7, "com.x.app", "App", "App", 2))
            .is_err());
        assert!(idx
            .publish(&signed(7, "com.x.app", "App", "App", 1))
            .is_err());
        // a newer version replaces it.
        idx.publish(&signed(7, "com.x.app", "App", "App", 3))
            .unwrap();
        assert_eq!(idx.list()[0].version, 3);
    }

    // ── id ownership (takeover) ──

    #[test]
    fn second_trusted_key_cannot_publish_over_the_first_publishers_id() {
        // Both 7 and 8 are trusted publishers, but com.x.app was first published by
        // 7. Being trusted for SOME id must not mean being trusted for EVERY id.
        let mut idx = index_trusting_all(&[7, 8]);
        let original = signed(7, "com.x.app", "App", "App", 1);
        idx.publish(&original).unwrap();

        for version in [1, 2, 99] {
            let err = idx
                .publish(&signed(8, "com.x.app", "App", "Hijacked", version))
                .expect_err("a different trusted key took over the id");
            assert!(err.contains("owned by"), "unclear error: {err}");
        }
        assert_eq!(idx.get("com.x.app"), Some(original.as_str()));
        assert_eq!(idx.list()[0].version, 1);
        assert_eq!(
            idx.owner("com.x.app"),
            Some(public_hex(&signing_key_from_hex(&key_hex(7)).unwrap()).as_str())
        );
    }

    #[test]
    fn original_publisher_can_still_publish_upgrades() {
        let mut idx = index_trusting_all(&[7, 8]);
        idx.publish(&signed(7, "com.x.app", "App", "App", 1))
            .unwrap();
        let _ = idx.publish(&signed(8, "com.x.app", "App", "App", 2)); // refused
        idx.publish(&signed(7, "com.x.app", "App", "App v2", 2))
            .unwrap();
        assert_eq!(idx.list()[0].version, 2);
        assert_eq!(idx.list()[0].name, "App v2");
        // ...and the second key can still publish its OWN ids.
        idx.publish(&signed(8, "com.y.other", "Other", "Other", 1))
            .unwrap();
        assert_eq!(idx.list().len(), 2);
    }

    // ── schema enforcement at publish ──

    #[test]
    fn publish_rejects_schema_invalid_manifests() {
        type Tamper = fn(&mut Manifest);
        let mut idx = index_trusting(7);
        let cases: [(&str, Tamper); 3] = [
            ("bad id", |m| m.id = "NotReverseDns".into()),
            ("non-semver version", |m| m.version = "1".into()),
            ("uppercase verb", |m| {
                m.needs = vec!["OPEN:obsidian://".into()]
            }),
        ];
        for (what, tamper) in cases {
            let mut m = manifest("com.x.app", "App", "App");
            tamper(&mut m);
            let err = idx
                .publish(&signed_manifest(7, m, 1))
                .expect_err(&format!("{what}: accepted a schema-invalid manifest"));
            assert!(err.contains("manifest"), "{what}: {err}");
        }
        assert!(idx.list().is_empty());
    }

    #[test]
    fn publish_rejects_a_manifest_with_an_unknown_field() {
        // The signature covers only the known manifest fields, so an injected field
        // still "verifies" — it must be refused rather than stored and served.
        let mut idx = index_trusting(7);
        let mut v: serde_json::Value =
            serde_json::from_str(&signed(7, "com.x.app", "App", "App", 1)).unwrap();
        v["manifest"]["permissions"] = serde_json::json!(["read:link-graph"]);
        let err = idx
            .publish(&v.to_string())
            .expect_err("unknown manifest field accepted");
        assert!(err.contains("unknown field"), "{err}");
        assert!(idx.get("com.x.app").is_none());
    }

    #[test]
    fn search_matches_id_name_and_app_case_insensitively() {
        let mut idx = index_trusting(7);
        idx.publish(&signed(7, "com.x.obsidian", "Obsidian", "Note Linker", 1))
            .unwrap();
        idx.publish(&signed(7, "com.x.finder", "Finder", "File Indexer", 1))
            .unwrap();
        assert_eq!(idx.search("obsidian").len(), 1);
        assert_eq!(idx.search("LINKER").len(), 1); // matches name "Note Linker"
        assert_eq!(idx.search("com.x").len(), 2); // both ids
        assert_eq!(idx.search("nothing").len(), 0);
    }

    /// In-memory Read+Write stream so the HTTP wire layer is testable without sockets.
    struct MockStream {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl std::io::Read for MockStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }
    impl std::io::Write for MockStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn mock(input: impl Into<Vec<u8>>) -> MockStream {
        MockStream {
            input: std::io::Cursor::new(input.into()),
            output: Vec::new(),
        }
    }
    /// Run one request through `handle_stream` with the default limits.
    fn roundtrip(input: impl Into<Vec<u8>>, idx: &Mutex<RegistryIndex>) -> String {
        let mut s = mock(input);
        handle_stream(&mut s, idx, &Limits::default()).unwrap();
        String::from_utf8(s.output).unwrap()
    }

    #[test]
    fn http_wire_layer_parses_a_post_then_serves_a_get() {
        // O4 wire layer (no sockets): a real HTTP/1.1 POST with a Content-Length body
        // publishes, and a follow-up GET returns the listing.
        let idx = Mutex::new(index_trusting(7));
        let bundle = signed(7, "com.x.app", "App", "Appy", 1);

        let post = format!(
            "POST /publish HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            bundle.len(),
            bundle
        );
        let resp = roundtrip(post, &idx);
        assert!(
            resp.starts_with("HTTP/1.1 201 Created"),
            "publish response: {resp}"
        );

        let resp = roundtrip("GET /integrations HTTP/1.1\r\n\r\n", &idx);
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("com.x.app"), "listing body: {resp}");
    }

    // ── request size caps ──

    /// Test-only global allocator that records the largest single allocation made
    /// on the current thread, so "413 before allocating" is checked directly.
    mod alloc_probe {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static LARGEST: Cell<usize> = const { Cell::new(0) };
        }
        fn note(size: usize) {
            let _ = LARGEST.try_with(|c| {
                if size > c.get() {
                    c.set(size)
                }
            });
        }
        pub fn reset() {
            LARGEST.with(|c| c.set(0));
        }
        pub fn largest() -> usize {
            LARGEST.with(|c| c.get())
        }

        pub struct Probe;
        unsafe impl GlobalAlloc for Probe {
            unsafe fn alloc(&self, l: Layout) -> *mut u8 {
                note(l.size());
                System.alloc(l)
            }
            unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
                note(l.size());
                System.alloc_zeroed(l)
            }
            unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
                System.dealloc(p, l)
            }
            unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
                note(new);
                System.realloc(p, l, new)
            }
        }
        #[global_allocator]
        static GLOBAL: Probe = Probe;
    }

    #[test]
    fn oversized_content_length_gets_413_without_allocating_the_body() {
        let idx = Mutex::new(index_trusting(7));
        // 4 GiB declared; only the headers are actually sent (a fake stream, not a
        // real 4 GiB upload). The old code did `vec![0u8; content_length]`.
        let req = "POST /publish HTTP/1.1\r\nContent-Length: 4294967296\r\n\r\n";
        alloc_probe::reset();
        let mut s = mock(req);
        let result = handle_stream(&mut s, &idx, &Limits::default());
        let largest = alloc_probe::largest();
        assert!(
            largest < 1024 * 1024,
            "allocated {largest} bytes for a request that should be refused"
        );
        result.unwrap();
        let resp = String::from_utf8(s.output).unwrap();
        assert!(resp.starts_with("HTTP/1.1 413 "), "{resp}");
    }

    #[test]
    fn content_length_just_over_the_cap_gets_413_and_at_the_cap_is_read() {
        let idx = Mutex::new(index_trusting(7));
        let limits = Limits {
            max_body: 64,
            ..Limits::default()
        };
        // one byte over: refused without reading the body.
        let mut s = mock(format!(
            "POST /publish HTTP/1.1\r\nContent-Length: 65\r\n\r\n{}",
            "x".repeat(65)
        ));
        handle_stream(&mut s, &idx, &limits).unwrap();
        let resp = String::from_utf8(s.output).unwrap();
        assert!(resp.starts_with("HTTP/1.1 413 "), "{resp}");
        // exactly at the cap: read and routed (400: it is not a real bundle).
        let mut s = mock(format!(
            "POST /publish HTTP/1.1\r\nContent-Length: 64\r\n\r\n{}",
            "x".repeat(64)
        ));
        handle_stream(&mut s, &idx, &limits).unwrap();
        let resp = String::from_utf8(s.output).unwrap();
        assert!(resp.starts_with("HTTP/1.1 400 "), "{resp}");
        assert!(
            resp.contains("malformed") || resp.contains("JSON"),
            "{resp}"
        );
    }

    #[test]
    fn body_past_content_length_is_never_buffered() {
        // A client that sends more than it declared cannot make the server buffer
        // the excess: exactly Content-Length bytes are consumed (plus at most one
        // read-ahead buffer), the rest is never read.
        let idx = Mutex::new(index_trusting(7));
        let limits = Limits {
            max_body: 64,
            ..Limits::default()
        };
        let mut s = mock(format!(
            "POST /publish HTTP/1.1\r\nContent-Length: 8\r\n\r\nnot json{}",
            "y".repeat(10 * 1024 * 1024)
        ));
        handle_stream(&mut s, &idx, &limits).unwrap();
        let resp = String::from_utf8(s.output).unwrap();
        assert!(resp.starts_with("HTTP/1.1 400 "), "{resp}");
        assert!(
            s.input.position() < 64 * 1024,
            "read {} bytes of an oversized body",
            s.input.position()
        );
    }

    #[test]
    fn malformed_framing_is_rejected() {
        let idx = Mutex::new(index_trusting(7));
        for (req, status) in [
            // unparseable / conflicting Content-Length must not silently become 0.
            (
                "POST /publish HTTP/1.1\r\nContent-Length: abc\r\n\r\n",
                "400",
            ),
            (
                "POST /publish HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nxx",
                "400",
            ),
            // truncated body (fewer bytes than declared, then EOF).
            (
                "POST /publish HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc",
                "400",
            ),
            // chunked bodies are not supported — refuse rather than mis-frame.
            (
                "POST /publish HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
                "501",
            ),
        ] {
            let resp = roundtrip(req, &idx);
            assert!(
                resp.starts_with(&format!("HTTP/1.1 {status} ")),
                "{req:?} -> {resp}"
            );
        }
    }

    #[test]
    fn oversized_request_line_and_headers_are_rejected() {
        let idx = Mutex::new(index_trusting(7));
        let limits = Limits::default();

        let long_path = format!(
            "GET /integrations?q={} HTTP/1.1\r\n\r\n",
            "a".repeat(limits.max_request_line)
        );
        let resp = roundtrip(long_path, &idx);
        assert!(resp.starts_with("HTTP/1.1 414 "), "{resp}");

        let big_header = format!(
            "GET /integrations HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(limits.max_header_bytes)
        );
        let resp = roundtrip(big_header, &idx);
        assert!(resp.starts_with("HTTP/1.1 431 "), "{resp}");

        // many small headers add up to the same cap.
        let many: String = (0..2000).map(|i| format!("X-{i}: v\r\n")).collect();
        let resp = roundtrip(format!("GET /integrations HTTP/1.1\r\n{many}\r\n"), &idx);
        assert!(resp.starts_with("HTTP/1.1 431 "), "{resp}");

        // an endless request line with no newline is cut off, not buffered forever.
        let mut s = mock("G".repeat(1024 * 1024));
        handle_stream(&mut s, &idx, &limits).unwrap();
        assert!(String::from_utf8(s.output)
            .unwrap()
            .starts_with("HTTP/1.1 414 "));
        assert!(s.input.position() as usize <= limits.max_request_line + 8192);
    }

    // ── no index lock during socket I/O ──

    /// A stream that checks, on every read and write, that the index lock is free.
    struct LockProbeStream {
        inner: MockStream,
        idx: Arc<Mutex<RegistryIndex>>,
        io_under_lock: usize,
    }
    impl LockProbeStream {
        fn check(&mut self) {
            if self.idx.try_lock().is_err() {
                self.io_under_lock += 1;
            }
        }
    }
    impl std::io::Read for LockProbeStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.check();
            self.inner.read(buf)
        }
    }
    impl std::io::Write for LockProbeStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.check();
            self.inner.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.check();
            Ok(())
        }
    }

    #[test]
    fn handle_stream_holds_no_index_lock_while_reading_or_writing() {
        let idx = Arc::new(Mutex::new(index_trusting(7)));
        let bundle = signed(7, "com.x.app", "App", "Appy", 1);
        for req in [
            format!(
                "POST /publish HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
                bundle.len(),
                bundle
            ),
            "GET /integrations/com.x.app HTTP/1.1\r\n\r\n".to_string(),
        ] {
            let mut s = LockProbeStream {
                inner: mock(req),
                idx: Arc::clone(&idx),
                io_under_lock: 0,
            };
            handle_stream(&mut s, &idx, &Limits::default()).unwrap();
            let resp = String::from_utf8(s.inner.output).unwrap();
            assert!(resp.starts_with("HTTP/1.1 20"), "{resp}");
            assert_eq!(s.io_under_lock, 0, "socket I/O happened under the lock");
        }
    }

    // ── real sockets: timeouts and the connection bound ──

    fn test_config() -> ServerConfig {
        ServerConfig {
            max_connections: 8,
            idle_timeout: Duration::from_secs(10),
            request_deadline: Duration::from_secs(20),
            ..ServerConfig::default()
        }
    }
    fn spawn_server(cfg: ServerConfig) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let idx = Arc::new(Mutex::new(index_trusting(7)));
        std::thread::spawn(move || serve(listener, idx, cfg));
        addr
    }
    /// A full GET that must answer within 3s (a hang fails the test, it can't wedge it).
    fn get_listing(addr: SocketAddr) -> std::io::Result<String> {
        let mut c = TcpStream::connect(addr)?;
        c.set_read_timeout(Some(Duration::from_secs(3)))?;
        c.write_all(b"GET /integrations HTTP/1.1\r\nHost: t\r\n\r\n")?;
        let mut out = String::new();
        c.read_to_string(&mut out)?;
        Ok(out)
    }

    #[test]
    fn an_idle_connection_does_not_block_a_concurrent_request() {
        let addr = spawn_server(test_config());
        // Connection 1 sends half a request and then goes quiet (slowloris-style).
        let mut idle = TcpStream::connect(addr).unwrap();
        idle.write_all(b"GET /integrations HTTP/1.1\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(200)); // let the server pick it up
                                                        // Connection 2 must still be served promptly.
        let started = Instant::now();
        let resp = get_listing(addr).expect("second request was blocked by the idle one");
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(idle);
    }

    #[test]
    fn connections_over_the_limit_get_503_and_slots_are_released() {
        let addr = spawn_server(ServerConfig {
            max_connections: 1,
            ..test_config()
        });
        // Occupy the only slot (the accept loop counts it before accepting the next).
        let mut hog = TcpStream::connect(addr).unwrap();
        hog.write_all(b"GET /").unwrap();

        let mut over = TcpStream::connect(addr).unwrap();
        over.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut out = String::new();
        over.read_to_string(&mut out)
            .expect("over-limit connection got no answer");
        assert!(out.starts_with("HTTP/1.1 503 "), "{out}");

        // Freeing the slot lets new connections through again.
        drop(hog);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let resp = get_listing(addr).unwrap_or_default();
            if resp.starts_with("HTTP/1.1 200 OK") {
                break;
            }
            assert!(Instant::now() < deadline, "slot never released: {resp}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Read until the server closes (Ok(0) / reset) or `limit` passes. True = closed.
    fn server_closed_within(c: &mut TcpStream, limit: Duration) -> bool {
        c.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let start = Instant::now();
        let mut buf = [0u8; 512];
        while start.elapsed() < limit {
            match c.read(&mut buf) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return true, // reset
            }
        }
        false
    }

    #[test]
    fn an_idle_client_is_disconnected_after_the_idle_timeout() {
        let addr = spawn_server(ServerConfig {
            idle_timeout: Duration::from_millis(300),
            ..test_config()
        });
        let mut c = TcpStream::connect(addr).unwrap();
        assert!(
            server_closed_within(&mut c, Duration::from_secs(3)),
            "idle connection was never closed"
        );
    }

    #[test]
    fn a_slow_drip_client_is_cut_off_by_the_request_deadline() {
        // Each byte arrives well inside the idle timeout, so only the overall
        // per-request deadline can stop this client from holding a slot forever.
        let addr = spawn_server(ServerConfig {
            idle_timeout: Duration::from_millis(500),
            request_deadline: Duration::from_secs(1),
            ..test_config()
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let start = Instant::now();
        let mut closed = false;
        let mut buf = [0u8; 512];
        while start.elapsed() < Duration::from_secs(5) {
            if c.write_all(b"G").is_err() {
                closed = true;
                break;
            }
            match c.read(&mut buf) {
                Ok(_) => {
                    closed = true;
                    break;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => {
                    closed = true;
                    break;
                }
            }
        }
        assert!(closed, "slow-drip connection was never cut off");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "cut off too late: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn router_lists_fetches_publishes_and_404s() {
        let mut idx = index_trusting(7);
        let bundle = signed(7, "com.x.app", "App", "Appy", 1);

        // publish via POST
        let r = handle_request("POST", "/publish", &bundle, &mut idx);
        assert_eq!(r.status, 201);

        // list
        let r = handle_request("GET", "/integrations", "", &mut idx);
        assert_eq!(r.status, 200);
        assert!(r.body.contains("com.x.app"));

        // search
        let r = handle_request("GET", "/integrations?q=appy", "", &mut idx);
        assert_eq!(r.status, 200);
        assert!(r.body.contains("com.x.app"));

        // fetch the verbatim signed bundle
        let r = handle_request("GET", "/integrations/com.x.app", "", &mut idx);
        assert_eq!(r.status, 200);
        assert!(r.body.contains("signature_hex"));

        // unknown id -> 404
        assert_eq!(
            handle_request("GET", "/integrations/nope", "", &mut idx).status,
            404
        );
        // bad publish -> 400 with an error body
        let r = handle_request("POST", "/publish", "not json", &mut idx);
        assert_eq!(r.status, 400);
        assert!(r.body.contains("error"));
        // unknown route -> 404
        assert_eq!(
            handle_request("DELETE", "/whatever", "", &mut idx).status,
            404
        );
    }
}
