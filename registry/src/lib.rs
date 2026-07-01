// Satisfies: O4 (a registry where signed integrations are published, discovered and
// fetched — the ecosystem distribution point B3's extension surface feeds into).
// See .manifold/linking-manifesto-app.md
//
// Functional-core / imperative-shell, same as the rest of ubilink: `RegistryIndex`
// (publish/get/list/search) and the pure `handle_request` router are 100% testable
// with no sockets; `bin/ubilink-registry.rs` is the thin std-only TcpListener shell.
//
// Trust reuses ubilink-core's proven model: a publish is accepted ONLY if the bundle
// verifies (ed25519, via ubilink_core::verify_json) against the registry's trusted
// publisher set, AND its version is strictly newer than what's already published
// (monotonicity — no silent downgrade of a published integration).

use std::collections::HashMap;

/// What the registry stores for a published integration id.
struct Entry {
    wire_json: String, // the exact signed bundle, served back verbatim
    version: u32,
    name: String,
    app: String,
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
    /// verifies against a trusted publisher AND is strictly newer than any existing
    /// version for the same id. Reuses ubilink-core's crypto verbatim (no reimpl).
    pub fn publish(&mut self, wire_json: &str) -> Result<(), String> {
        // 1) crypto + trusted-publisher check (the R3/R8 proven path).
        ubilink_core::verify_json(wire_json, &self.trusted_pub_hex).map_err(|e| e.message())?;
        // 2) parse for id/version/name/app (WireBundle is the public wire format).
        let wire: ubilink_core::WireBundle =
            serde_json::from_str(wire_json).map_err(|e| format!("malformed bundle JSON: {e}"))?;
        // 3) monotonicity: never replace a published version with an older/equal one.
        if let Some(cur) = self.by_id.get(&wire.manifest.id) {
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
            },
        );
        Ok(())
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
        Resp {
            status: 404,
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

/// Serve ONE HTTP/1.1 request on a stream: parse request line + Content-Length body,
/// route via `handle_request`, write the response. Generic over Read+Write so it is
/// unit-tested with an in-memory stream (no sockets); the bin passes a `TcpStream`.
pub fn handle_stream<S: std::io::Read + std::io::Write>(
    stream: &mut S,
    idx: &mut RegistryIndex,
) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Read};
    let mut reader = BufReader::new(&mut *stream);

    // request line
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // headers — we only need Content-Length.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }

    // body (exactly Content-Length bytes)
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    let resp = handle_request(&method, &path, &body, idx);
    let reason = match resp.status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        resp.status, reason, resp.body.len(), resp.body
    );
    stream.write_all(out.as_bytes())?;
    stream.flush()
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
            version: "1".into(),
            needs: vec![],
            signature: None,
        }
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

    #[test]
    fn http_wire_layer_parses_a_post_then_serves_a_get() {
        // O4 wire layer (no sockets): a real HTTP/1.1 POST with a Content-Length body
        // publishes, and a follow-up GET returns the listing.
        let mut idx = index_trusting(7);
        let bundle = signed(7, "com.x.app", "App", "Appy", 1);

        let post = format!(
            "POST /publish HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            bundle.len(),
            bundle
        );
        let mut s = MockStream {
            input: std::io::Cursor::new(post.into_bytes()),
            output: Vec::new(),
        };
        handle_stream(&mut s, &mut idx).unwrap();
        let resp = String::from_utf8(s.output).unwrap();
        assert!(
            resp.starts_with("HTTP/1.1 201 Created"),
            "publish response: {resp}"
        );

        let mut s = MockStream {
            input: std::io::Cursor::new(b"GET /integrations HTTP/1.1\r\n\r\n".to_vec()),
            output: Vec::new(),
        };
        handle_stream(&mut s, &mut idx).unwrap();
        let resp = String::from_utf8(s.output).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("com.x.app"), "listing body: {resp}");
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
