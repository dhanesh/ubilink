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
// that first published it for the life of the data directory (`RegistryIndex::open`),
// or of the process for a purely in-memory index (`RegistryIndex::new`).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What the registry stores for a published integration id.
struct Entry {
    wire_json: String, // the exact signed bundle, served back verbatim
    version: u32,
    name: String,
    app: String,
    /// Lowercase hex of the ed25519 key that first published this id. Persisted in
    /// the id's record next to the bundle, so ownership survives a restart.
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

/// Why a publish was refused: the bundle's fault (HTTP 400) or the store's (500).
enum PublishError {
    Rejected(String),
    Storage(String),
}

impl PublishError {
    fn message(self) -> String {
        match self {
            PublishError::Rejected(m) | PublishError::Storage(m) => m,
        }
    }
}

/// The registry's index. Always in memory; when opened on a data directory
/// (`open`) it is also on disk, one record per id (see `Store`). A purely
/// in-memory index (`new`) forgets every entry AND its owner on restart.
pub struct RegistryIndex {
    trusted_pub_hex: Vec<String>,
    by_id: HashMap<String, Entry>,
    /// Ids whose stored record did not load at startup, with the reason. They are
    /// neither served nor publishable by ANY key (fail closed) until an operator
    /// restores or removes the record — see `load`.
    reserved: HashMap<String, String>,
    store: Option<Store>,
}

impl RegistryIndex {
    /// An in-memory index: nothing is persisted.
    pub fn new(trusted_pub_hex: Vec<String>) -> RegistryIndex {
        RegistryIndex {
            trusted_pub_hex,
            by_id: HashMap::new(),
            reserved: HashMap::new(),
            store: None,
        }
    }

    /// An index backed by `data_dir` (created if missing). It is rebuilt from the
    /// records already there, each re-verified against `trusted_pub_hex`, and every
    /// later accepted publish is written there before it is applied. Fails if
    /// another index (in this or another process) holds the directory.
    pub fn open(
        trusted_pub_hex: Vec<String>,
        data_dir: impl AsRef<Path>,
    ) -> io::Result<RegistryIndex> {
        let dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let lock = Store::lock(&dir)?;
        let mut idx = RegistryIndex::new(trusted_pub_hex);
        idx.load(&dir)?;
        idx.store = Some(Store {
            dir,
            _lock: lock,
            failed: None,
        });
        Ok(idx)
    }

    /// Rebuild the index from `dir`. Temp files (an interrupted write) and dotfiles
    /// (the lock, OS metadata) are skipped. A record that does not pass the same
    /// gate as a publish is NOT served, and its id is RESERVED: publishable by no
    /// key until an operator acts. Fail closed, because once a record fails to
    /// verify we cannot tell a revoked publisher from a tampered or corrupt file,
    /// nor trust its owner/version fields — and freeing the id would let any other
    /// trusted key claim it, which is exactly what ownership exists to prevent.
    fn load(&mut self, dir: &Path) -> io::Result<()> {
        let mut names = Vec::new();
        for ent in fs::read_dir(dir)? {
            match ent?.file_name().into_string() {
                Ok(n) => names.push(n),
                Err(n) => warn(&format!("ignoring {n:?} in the data dir: not UTF-8")),
            }
        }
        names.sort(); // deterministic load order and warnings
        for name in names {
            if name.starts_with('.') {
                continue;
            }
            if name.ends_with(TEMP_SUFFIX) {
                warn(&format!(
                    "ignoring leftover temp file {name}: an interrupted write that was \
                     never acknowledged (the previous record, if any, is intact)"
                ));
                continue;
            }
            let path = dir.join(&name);
            let Some(id) = name.strip_suffix(RECORD_EXT).filter(|id| is_safe_id(id)) else {
                warn(&format!(
                    "ignoring {}: not an `<integration-id>{RECORD_EXT}` record",
                    path.display()
                ));
                continue;
            };
            match self.load_record(&path, id) {
                Ok(entry) => {
                    self.by_id.insert(id.to_string(), entry);
                }
                Err(why) => {
                    warn(&format!(
                        "NOT serving {id}: {}. The id stays reserved (no key can \
                         publish it) until an operator restores or removes {}",
                        why.trim_end_matches('.'),
                        path.display()
                    ));
                    self.reserved.insert(id.to_string(), why);
                }
            }
        }
        Ok(())
    }

    /// One stored record through the same gate a publish passes (`verified`, with
    /// the CURRENT trusted keys), plus the checks that tie it to its file: the
    /// bundle is for `id` and was signed by the recorded owner. The latest bundle
    /// of an id is always signed by its owner (only the owner may publish upgrades
    /// and there is no rotation), so the two can never legitimately differ.
    fn load_record(&self, path: &Path, id: &str) -> Result<Entry, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("unreadable record: {e}"))?;
        let (owner_hex, wire_json) = parse_record(&text)?;
        let (bundle_id, entry) = self
            .verified(&wire_json)
            .map_err(|e| format!("stored bundle does not verify: {e}"))?;
        if bundle_id != id {
            return Err(format!(
                "the record file is for {id:?} but its bundle is for {bundle_id:?}"
            ));
        }
        if entry.owner_hex != owner_hex {
            return Err(format!(
                "recorded owner {owner_hex} is not the bundle's signer {}",
                entry.owner_hex
            ));
        }
        Ok(entry)
    }

    /// Ids held back at startup because their stored record did not load (sorted).
    pub fn reserved_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.reserved.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Publish a signed bundle (the JSON `ubilink-sign sign` produces). Accepted iff it
    /// verifies against a trusted publisher with a schema-valid manifest, the id is
    /// new or owned by the same signer, AND it is strictly newer than any existing
    /// version for the same id. Reuses ubilink-core's crypto verbatim (no reimpl).
    /// With a data directory, the record is persisted BEFORE the index changes; if
    /// that fails the publish fails and the index is untouched.
    pub fn publish(&mut self, wire_json: &str) -> Result<(), String> {
        self.try_publish(wire_json).map_err(PublishError::message)
    }

    fn try_publish(&mut self, wire_json: &str) -> Result<(), PublishError> {
        // 1) + 2) signature, trusted publisher, manifest schema; parse.
        let (id, entry) = self.verified(wire_json).map_err(PublishError::Rejected)?;
        // 3) an id whose stored record did not load is publishable by no one.
        if let Some(why) = self.reserved.get(&id) {
            return Err(PublishError::Rejected(format!(
                "integration id {id:?} is reserved: its stored record did not load at \
                 startup ({why}); an operator must restore or remove that record before \
                 this id can be published again"
            )));
        }
        if let Some(cur) = self.by_id.get(&id) {
            // 4) ownership: only the key that first published an id may update it.
            if cur.owner_hex != entry.owner_hex {
                return Err(PublishError::Rejected(format!(
                    "integration id {id:?} is owned by publisher key {}; a bundle signed by \
                     {} cannot replace it (ids are bound to their first publisher; key \
                     rotation is not supported yet)",
                    cur.owner_hex, entry.owner_hex
                )));
            }
            // 5) monotonicity: never replace a published version with an older/equal one.
            if entry.version <= cur.version {
                return Err(PublishError::Rejected(format!(
                    "version {} is not newer than the published version {}",
                    entry.version, cur.version
                )));
            }
        }
        // 6) durability: write the record first, so memory never holds a publish
        //    the disk lacks. On failure nothing below runs and the index is unchanged.
        if let Some(store) = self.store.as_mut() {
            store.persist(&id, &entry).map_err(PublishError::Storage)?;
        }
        self.by_id.insert(id, entry);
        Ok(())
    }

    /// The gate every bundle passes, on publish and again on load: crypto +
    /// trusted publisher + manifest schema (the R3/R8 proven path;
    /// `Installer::verify` runs `Manifest::validate`, and serde refuses unknown
    /// manifest fields), then a parse for id/version/name/app/signer.
    fn verified(&self, wire_json: &str) -> Result<(String, Entry), String> {
        ubilink_core::verify_json(wire_json, &self.trusted_pub_hex).map_err(|e| e.message())?;
        let wire: ubilink_core::WireBundle =
            serde_json::from_str(wire_json).map_err(|e| format!("malformed bundle JSON: {e}"))?;
        // verify_json accepted signer_hex as 64 hex chars (surrounding whitespace
        // tolerated, either case) — normalise so ownership compares key bytes.
        let owner_hex = wire.signer_hex.trim().to_ascii_lowercase();
        let m = wire.manifest;
        Ok((
            m.id,
            Entry {
                wire_json: wire_json.to_string(),
                version: wire.version,
                name: m.name,
                app: m.app,
                owner_hex,
            },
        ))
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
        "POST" if path == "/publish" => match idx.try_publish(body) {
            Ok(()) => Resp::created(r#"{"published":true}"#.to_string()),
            Err(PublishError::Rejected(e)) => Resp::bad(&e),
            Err(PublishError::Storage(e)) => Resp::error(500, &e),
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
        500 => "Internal Server Error",
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
/// operation, release it, then write the response.
///
/// With a data directory, an accepted publish writes (and fsyncs) its record while
/// the lock is held. That is deliberate: check-ownership, persist and apply must be
/// one atomic step, or two concurrent first publishes of an id by different keys
/// could both pass the check and the later rename would silently replace the first
/// owner on disk. The write is one small file (a bundle is a few KiB, capped at
/// 1 MiB), so the lock is held for roughly one fsync; reads wait that long at
/// most, which suits a low-traffic registry. Socket I/O still never happens under
/// the lock. Generic over Read+Write so it
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
            // every check has passed and the record is persisted, in a single insert.
            let mut guard = idx.lock().unwrap_or_else(PoisonError::into_inner);
            handle_request(&req.method, &req.path, &req.body, &mut guard)
        } // lock released here, before any socket write
    };
    write_response(stream, &resp)
}

// ── durable store ──
//
// Layout: one file per integration id, `<data-dir>/<id>.json`:
//   {"format": 1, "owner_hex": "<64 lowercase hex>", "wire_json": "<signed bundle>"}
// `wire_json` is the exact published bytes as a JSON string (served back verbatim).
// Ids are schema-checked lowercase reverse-DNS names, so they are safe and
// case-unambiguous file names; `record_path` re-checks that anyway. `.lock` holds
// the single-writer lock; `<id>.json.tmp` exists only while a write is in flight.
//
// Write protocol (`Store::persist`): write `<id>.json.tmp` and `sync_all` it,
// `rename` it over `<id>.json` (atomic on POSIX: a reader or a crash sees the whole
// old record or the whole new one), then fsync the directory so the rename itself
// survives a power loss.

const STORE_FORMAT: u64 = 1;
const RECORD_EXT: &str = ".json";
const TEMP_SUFFIX: &str = ".tmp";
const LOCK_FILE: &str = ".lock";

struct Store {
    dir: PathBuf,
    /// Exclusive advisory lock on `<dir>/.lock` for the store's life (released on
    /// drop or process exit): one writer per directory, so no second index can
    /// make ownership decisions from a stale memory and overwrite an owner.
    _lock: File,
    /// Set once a record reached the directory but the directory fsync failed, so
    /// memory (unchanged) and disk (new record) may differ. Every later publish is
    /// refused until a restart reloads from disk.
    failed: Option<String>,
}

impl Store {
    fn lock(dir: &Path) -> io::Result<File> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK_FILE))?;
        match f.try_lock() {
            Ok(()) => Ok(f),
            Err(fs::TryLockError::WouldBlock) => Err(io::Error::other(format!(
                "data directory {} is already in use by another registry index (it \
                 holds {LOCK_FILE})",
                dir.display()
            ))),
            Err(fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// Durably write `id`'s record. Errors are client-facing (no paths); details go
    /// to stderr.
    fn persist(&mut self, id: &str, entry: &Entry) -> Result<(), String> {
        if let Some(why) = &self.failed {
            return Err(format!(
                "publishing is disabled until the registry restarts: an earlier write \
                 could not be confirmed durable ({why})"
            ));
        }
        let path = record_path(&self.dir, id)?;
        let tmp = self.dir.join(format!("{id}{RECORD_EXT}{TEMP_SUFFIX}"));
        let record = serde_json::json!({
            "format": STORE_FORMAT,
            "owner_hex": entry.owner_hex,
            "wire_json": entry.wire_json,
        });
        let bytes = format!("{record:#}\n");
        if let Err(e) = write_synced(&tmp, bytes.as_bytes()).and_then(|()| fs::rename(&tmp, &path))
        {
            // Nothing was replaced: the old record (if any) is intact.
            let _ = fs::remove_file(&tmp);
            warn(&format!("could not persist {}: {e}", path.display()));
            return Err(format!("could not persist {id}: {e}"));
        }
        // The new record is in place. Until the directory is synced, a power loss
        // could still undo the rename.
        if let Err(e) = sync_dir(&self.dir) {
            let why = format!("fsync of the data dir after writing {id} failed: {e}");
            warn(&format!("{why}; refusing further publishes until restart"));
            self.failed = Some(why.clone());
            return Err(format!("could not persist {id}: {why}"));
        }
        Ok(())
    }
}

/// Create `path` afresh (a stale file, or a symlink planted in its place, is
/// removed first; `create_new` never follows one), write `bytes`, and flush them to
/// stable storage (`sync_all`; `F_FULLFSYNC` on Apple platforms).
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// fsync a directory so a rename inside it is durable. On Unix std opens a
/// directory as a read-only `File` and `sync_all` fsyncs it (`F_FULLFSYNC` on
/// Apple platforms). A filesystem that cannot fsync a directory at all
/// (EINVAL/ENOTSUP) leaves nothing more to do.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir)?.sync_all() {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        r => r,
    }
}

/// std cannot open a directory handle on Windows (that needs
/// FILE_FLAG_BACKUP_SEMANTICS), so there is no portable directory fsync; NTFS
/// journals the rename's metadata.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// The record file for `id` inside `dir`. Anything that is not a plain
/// reverse-DNS id is refused, so a path built from it can never leave `dir`.
fn record_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    let name = format!("{id}{RECORD_EXT}");
    let mut parts = Path::new(&name).components();
    match (is_safe_id(id), parts.next(), parts.next()) {
        (true, Some(std::path::Component::Normal(n)), None) if n == name.as_str() => {
            Ok(dir.join(name))
        }
        _ => Err(format!(
            "refusing to store integration id {id:?}: not a lowercase reverse-DNS id"
        )),
    }
}

/// The schema's id pattern `^[a-z0-9]+(\.[a-z0-9-]+)+$`, re-checked here because
/// the id becomes a file name (`Manifest::validate` already enforced it on publish;
/// this guards load and any future caller). Path separators and `..` are refused
/// explicitly as well, though the pattern already excludes them.
fn is_safe_id(id: &str) -> bool {
    if id.contains(['/', '\\']) || id.contains("..") {
        return false;
    }
    let label_ok = |l: &str, dash: bool| {
        !l.is_empty()
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || (dash && b == b'-'))
    };
    let mut labels = id.split('.');
    let first_ok = labels.next().is_some_and(|l| label_ok(l, false));
    let rest: Vec<&str> = labels.collect();
    first_ok && !rest.is_empty() && rest.iter().all(|l| label_ok(l, true))
}

/// `(owner_hex, wire_json)` from a stored record.
fn parse_record(text: &str) -> Result<(String, String), String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("corrupt record: {e}"))?;
    let format = v.get("format").and_then(serde_json::Value::as_u64);
    if format != Some(STORE_FORMAT) {
        return Err(format!(
            "unsupported record format {format:?} (this registry reads {STORE_FORMAT})"
        ));
    }
    let field = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("corrupt record: missing string field `{k}`"))
    };
    Ok((field("owner_hex")?, field("wire_json")?))
}

fn warn(msg: &str) {
    eprintln!("[ubilink-registry] warning: {msg}");
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
        spawn_server_with(Arc::new(Mutex::new(index_trusting(7))), cfg)
    }
    fn spawn_server_with(idx: Arc<Mutex<RegistryIndex>>, cfg: ServerConfig) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
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

    // ── durable store ──

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn pubkey(seed: u8) -> String {
        public_hex(&signing_key_from_hex(&key_hex(seed)).unwrap())
    }
    fn trusting(seeds: &[u8]) -> Vec<String> {
        seeds.iter().map(|s| pubkey(*s)).collect()
    }

    /// A unique scratch directory per test, removed on drop (even if a test turned
    /// it into a plain file).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "ubilink-registry-test-{}-{}-{tag}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn record(&self, id: &str) -> PathBuf {
            self.0.join(format!("{id}.json"))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            if fs::remove_dir_all(&self.0).is_err() {
                let _ = fs::remove_file(&self.0);
            }
        }
    }

    fn open(seeds: &[u8], dir: &TempDir) -> RegistryIndex {
        RegistryIndex::open(trusting(seeds), dir.path()).unwrap()
    }
    fn edit_record(path: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        let mut rec: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        change(&mut rec);
        fs::write(path, rec.to_string()).unwrap();
    }

    #[test]
    fn a_published_entry_and_its_owner_survive_a_reload() {
        let dir = TempDir::new("reload");
        let bundle = signed(7, "com.x.obsidian", "Obsidian", "Obsidian Linker", 1);
        let mut idx = open(&[7], &dir);
        idx.publish(&bundle).unwrap();
        drop(idx);

        // One file per id: the owner key next to the verbatim signed bundle.
        let rec: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.record("com.x.obsidian")).unwrap())
                .unwrap();
        assert_eq!(rec["owner_hex"], pubkey(7));
        assert_eq!(rec["wire_json"], bundle.as_str());

        let idx = open(&[7], &dir);
        assert_eq!(idx.get("com.x.obsidian"), Some(bundle.as_str()));
        assert_eq!(idx.owner("com.x.obsidian"), Some(pubkey(7).as_str()));
        let list = idx.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Obsidian Linker");
        assert_eq!(list[0].version, 1);
    }

    #[test]
    fn after_a_reload_a_second_trusted_key_still_cannot_take_over_an_id() {
        // The PR #9 ownership rule must survive a restart: before persistence, the
        // reloaded index was empty and key 8's first publish claimed the id.
        let dir = TempDir::new("takeover");
        let original = signed(7, "com.x.app", "App", "App", 1);
        open(&[7, 8], &dir).publish(&original).unwrap();

        let mut idx = open(&[7, 8], &dir); // a restart
        for version in [1, 2, 99] {
            let err = idx
                .publish(&signed(8, "com.x.app", "App", "Hijacked", version))
                .expect_err("after a restart a different trusted key took over the id");
            assert!(err.contains("owned by"), "unclear error: {err}");
        }
        assert_eq!(idx.get("com.x.app"), Some(original.as_str()));
        assert_eq!(idx.owner("com.x.app"), Some(pubkey(7).as_str()));
        drop(idx);

        // The refused attempts never reached disk.
        let idx = open(&[7, 8], &dir);
        assert_eq!(idx.get("com.x.app"), Some(original.as_str()));
        assert_eq!(idx.owner("com.x.app"), Some(pubkey(7).as_str()));
    }

    #[test]
    fn after_a_reload_the_owner_can_publish_upgrades_and_the_newest_wins() {
        let dir = TempDir::new("upgrade");
        open(&[7, 8], &dir)
            .publish(&signed(7, "com.x.app", "App", "App", 1))
            .unwrap();

        let mut idx = open(&[7, 8], &dir);
        let v2 = signed(7, "com.x.app", "App", "App v2", 2);
        idx.publish(&v2).unwrap();
        assert_eq!(idx.list()[0].version, 2);
        drop(idx);

        let mut idx = open(&[7, 8], &dir);
        assert_eq!(idx.get("com.x.app"), Some(v2.as_str()));
        assert_eq!(idx.list()[0].name, "App v2");
        // monotonicity is enforced against the reloaded version too.
        for old in [1, 2] {
            assert!(idx
                .publish(&signed(7, "com.x.app", "App", "App", old))
                .is_err());
        }
    }

    #[test]
    fn a_leftover_temp_file_from_an_interrupted_write_is_ignored() {
        let dir = TempDir::new("tmp");
        let v1 = signed(7, "com.x.app", "App", "App", 1);
        open(&[7, 8], &dir).publish(&v1).unwrap();

        // A crash mid-write leaves `<id>.json.tmp`: a torn one for an existing id,
        // and a complete one for an id whose publish died before the rename (so it
        // was never acknowledged, and never happened).
        fs::write(
            dir.path().join("com.x.app.json.tmp"),
            r#"{"format":1,"owner_hex":"ab"#,
        )
        .unwrap();
        let never_acked = serde_json::json!({
            "format": 1,
            "owner_hex": pubkey(7),
            "wire_json": signed(7, "com.y.other", "O", "O", 1),
        });
        fs::write(
            dir.path().join("com.y.other.json.tmp"),
            never_acked.to_string(),
        )
        .unwrap();

        let mut idx = open(&[7, 8], &dir);
        assert_eq!(idx.get("com.x.app"), Some(v1.as_str()));
        assert_eq!(idx.list().len(), 1);
        assert!(idx.get("com.y.other").is_none());
        // Not reserved either: the unacknowledged claim gives key 7 nothing.
        idx.publish(&signed(8, "com.y.other", "O", "O", 1)).unwrap();
        // The owner's next write replaces the stale temp file cleanly.
        let v2 = signed(7, "com.x.app", "App", "App", 2);
        idx.publish(&v2).unwrap();
        drop(idx);

        let idx = open(&[7, 8], &dir);
        assert_eq!(idx.get("com.x.app"), Some(v2.as_str()));
        assert_eq!(idx.owner("com.y.other"), Some(pubkey(8).as_str()));
        assert!(!dir.path().join("com.x.app.json.tmp").exists());
        assert!(!dir.path().join("com.y.other.json.tmp").exists());
    }

    #[test]
    fn a_stored_record_that_no_longer_verifies_is_not_served_and_its_id_stays_reserved() {
        type Tamper = fn(&Path);
        let cases: [(&str, &[u8], Tamper); 6] = [
            ("edited wire_json", &[7, 8], |p| {
                edit_record(p, |r| {
                    let w = r["wire_json"].as_str().unwrap();
                    r["wire_json"] = w.replace("run(){}", "run(){steal()}").into();
                })
            }),
            ("truncated file", &[7, 8], |p| {
                fs::write(p, r#"{"format":1,"owner_"#).unwrap()
            }),
            ("owner is not the signer", &[7, 8], |p| {
                edit_record(p, |r| r["owner_hex"] = pubkey(8).into())
            }),
            ("file holds another id's bundle", &[7, 8], |p| {
                edit_record(p, |r| {
                    r["wire_json"] = signed(7, "com.x.other", "App", "App", 1).into()
                })
            }),
            ("unknown record format", &[7, 8], |p| {
                edit_record(p, |r| r["format"] = 2.into())
            }),
            // Revoking trust hides the owner's bundle but must not free its id.
            ("owner key removed from trust", &[8], |_| {}),
        ];
        for (what, trust, tamper) in cases {
            let dir = TempDir::new("tamper");
            open(&[7, 8], &dir)
                .publish(&signed(7, "com.x.app", "App", "App", 1))
                .unwrap();
            let file = dir.record("com.x.app");
            tamper(&file);
            let evidence = fs::read(&file).unwrap();

            let mut idx = open(trust, &dir);
            assert!(idx.get("com.x.app").is_none(), "{what}: served");
            assert!(idx.list().is_empty(), "{what}: listed");
            assert_eq!(idx.owner("com.x.app"), None, "{what}");
            for seed in [7u8, 8] {
                let err = idx
                    .publish(&signed(seed, "com.x.app", "App", "Claimed", 2))
                    .expect_err(&format!("{what}: key {seed} published a reserved id"));
                if trust.contains(&seed) {
                    assert!(err.contains("reserved"), "{what}: {err}");
                }
            }
            assert_eq!(
                fs::read(&file).unwrap(),
                evidence,
                "{what}: the stored record was overwritten"
            );
            // Other ids are unaffected.
            idx.publish(&signed(8, "com.y.other", "O", "O", 1)).unwrap();
        }
    }

    #[test]
    fn a_failed_persist_fails_the_publish_and_leaves_memory_unchanged() {
        // (a) The data dir vanishes and a plain file takes its place.
        let dir = TempDir::new("persist-fail");
        let v1 = signed(7, "com.x.app", "App", "App", 1);
        let mut idx = open(&[7], &dir);
        idx.publish(&v1).unwrap();
        fs::remove_dir_all(dir.path()).unwrap();
        fs::write(dir.path(), "not a directory").unwrap();

        let err = idx
            .publish(&signed(7, "com.x.app", "App", "App v2", 2))
            .expect_err("an upgrade was accepted without being persisted");
        assert!(err.contains("persist"), "{err}");
        assert_eq!(idx.get("com.x.app"), Some(v1.as_str()));
        assert_eq!(idx.list()[0].version, 1);
        // Over HTTP a storage failure is the server's fault: 500, not 400.
        let r = handle_request(
            "POST",
            "/publish",
            &signed(7, "com.y.new", "N", "N", 1),
            &mut idx,
        );
        assert_eq!(r.status, 500, "{r:?}");
        assert!(idx.get("com.y.new").is_none());
        assert_eq!(idx.list().len(), 1);

        // (b) The record path is taken by a directory, so the rename fails.
        let dir = TempDir::new("persist-fail-rename");
        let mut idx = open(&[7, 8], &dir);
        let blocker = dir.record("com.x.app");
        fs::create_dir(&blocker).unwrap();
        fs::write(blocker.join("keep"), "x").unwrap();
        assert!(idx
            .publish(&signed(7, "com.x.app", "App", "App", 1))
            .is_err());
        assert!(idx.get("com.x.app").is_none());
        assert_eq!(idx.owner("com.x.app"), None);
        assert!(
            !dir.path().join("com.x.app.json.tmp").exists(),
            "temp file left behind"
        );
        // The failed claim reserved nothing.
        fs::remove_dir_all(&blocker).unwrap();
        idx.publish(&signed(8, "com.x.app", "App", "App", 1))
            .unwrap();
        assert_eq!(idx.owner("com.x.app"), Some(pubkey(8).as_str()));
    }

    #[test]
    fn record_paths_never_leave_the_data_dir() {
        let dir = Path::new("/srv/registry-data");
        let p = record_path(dir, "com.x-y.app2").unwrap();
        assert_eq!(p, dir.join("com.x-y.app2.json"));
        assert_eq!(p.parent(), Some(dir));
        for bad in [
            "",
            ".",
            "..",
            "../com.x.app",
            "com.x/../../etc",
            "com.x.app/..",
            "com.x/app",
            "/etc/passwd.x",
            "com\\x.app",
            "..\\com.x",
            "c:\\x.y",
            "com..x",
            ".com.x",
            "com.x.",
            "COM.X.APP",
            "com",
            "-com.x",
            "com.x.app\0",
            "com.x app",
        ] {
            assert!(record_path(dir, bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_data_dir_is_used_by_one_index_at_a_time() {
        // Two live indexes on one directory would each decide ownership from their
        // own memory, and the second could overwrite the first's owner on disk.
        let dir = TempDir::new("lock");
        let first = open(&[7], &dir);
        let err = RegistryIndex::open(trusting(&[7]), dir.path())
            .err()
            .expect("two indexes opened the same data dir");
        assert!(err.to_string().contains("in use"), "{err}");
        drop(first);
        open(&[7], &dir);
    }

    fn post(bundle: &str) -> String {
        format!(
            "POST /publish HTTP/1.1\r\nContent-Length: {}\r\n\r\n{bundle}",
            bundle.len()
        )
    }
    fn http(addr: SocketAddr, req: &str) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serve_keeps_published_bundles_across_a_restart_on_the_same_data_dir() {
        let dir = TempDir::new("e2e");
        let bundle = signed(7, "com.x.app", "App", "Appy", 1);
        let first = Arc::new(Mutex::new(open(&[7, 8], &dir)));
        let addr = spawn_server_with(Arc::clone(&first), test_config());
        let resp = http(addr, &post(&bundle));
        assert!(resp.starts_with("HTTP/1.1 201 "), "{resp}");

        // Stop the first server: drop its index (and its data-dir lock), so nothing
        // it held in memory can reach the second one.
        drop(std::mem::replace(
            &mut *first.lock().unwrap(),
            RegistryIndex::new(vec![]),
        ));

        let second = Arc::new(Mutex::new(open(&[7, 8], &dir)));
        let addr = spawn_server_with(second, test_config());
        let resp = http(addr, "GET /integrations/com.x.app HTTP/1.1\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
        assert!(
            resp.ends_with(&bundle),
            "bundle not served verbatim: {resp}"
        );
        // ...and ownership came back with it.
        let hijack = signed(8, "com.x.app", "App", "Hijacked", 2);
        let resp = http(addr, &post(&hijack));
        assert!(
            resp.starts_with("HTTP/1.1 400 ") && resp.contains("owned by"),
            "{resp}"
        );
    }
}
