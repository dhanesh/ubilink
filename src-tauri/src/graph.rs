// Satisfies: RT-4 (persistent bidirectional link graph), T2 (local store, atomic
// & consistent, thread-safe), GAP-17 (concurrency), U5 (backlinks visible from
// both ends).
// See .manifold/linking-manifesto-app.md
//
// The link graph. This is what makes ubilink a *linking tool* rather than a
// clipboard helper (RT-4): linking A<->B is stored ONCE, and the link is queryable
// as a backlink from BOTH endpoints (U5). Storage is local SQLite (T2 / S2:
// on-device), with WAL mode + a UNIQUE canonical-pair constraint so concurrent
// captures (GAP-17) are serialized and idempotent rather than duplicating links.

use crate::resource::{Link, ResourceIdentity};
use rusqlite::{params, Connection};
use std::collections::HashMap;

pub struct LinkGraph {
    conn: Connection,
}

impl LinkGraph {
    /// Open (and migrate) a graph at `path`. Persistent (RT-4).
    pub fn open(path: &str) -> rusqlite::Result<LinkGraph> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// In-memory graph, for tests that don't need persistence.
    pub fn open_in_memory() -> rusqlite::Result<LinkGraph> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<LinkGraph> {
        // WAL: concurrent readers + a serialized writer (GAP-17). busy_timeout so a
        // racing writer waits for the lock instead of erroring.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS resources (
                id           TEXT PRIMARY KEY,
                name         TEXT NOT NULL,
                uri          TEXT NOT NULL,
                path         TEXT,
                content_hash TEXT,
                guid         TEXT,     -- stable short id (derived from uri); used by export/import
                created_at   INTEGER,  -- first-capture time (ms, caller-supplied — core has no clock)
                updated_at   INTEGER   -- last-capture time; list orders by this so re-capture resurfaces
            );
            CREATE TABLE IF NOT EXISTS links (
                id         TEXT PRIMARY KEY,
                lo_id      TEXT NOT NULL REFERENCES resources(id),
                hi_id      TEXT NOT NULL REFERENCES resources(id),
                created_at INTEGER,
                -- one row per unordered pair: linking A<->B and B<->A is the SAME link
                UNIQUE (lo_id, hi_id)
            );
            CREATE INDEX IF NOT EXISTS idx_links_lo ON links(lo_id);
            CREATE INDEX IF NOT EXISTS idx_links_hi ON links(hi_id);
            "#,
        )?;
        Self::migrate(&conn)?;
        Ok(LinkGraph { conn })
    }

    /// Add guid/created_at/updated_at to a pre-existing resources table (DBs created
    /// before these columns). Backfills order from rowid (so the current list order
    /// is preserved; real ms-timestamps from future captures sort above all legacy
    /// rows) and guid from the URI.
    fn migrate(conn: &Connection) -> rusqlite::Result<()> {
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('resources')")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        if !cols.iter().any(|c| c == "guid") {
            conn.execute("ALTER TABLE resources ADD COLUMN guid TEXT", [])?;
            conn.execute("ALTER TABLE resources ADD COLUMN created_at INTEGER", [])?;
            conn.execute("ALTER TABLE resources ADD COLUMN updated_at INTEGER", [])?;
            conn.execute(
                "UPDATE resources SET created_at = rowid, updated_at = rowid",
                [],
            )?;
            // each legacy row gets a fresh UUID (stable once written).
            let ids: Vec<String> = {
                let mut stmt = conn.prepare("SELECT id FROM resources WHERE guid IS NULL")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for id in ids {
                conn.execute(
                    "UPDATE resources SET guid = ?1 WHERE id = ?2",
                    params![Self::new_guid(), id],
                )?;
            }
            // The first backfill used rowid as a placeholder created_at — but rowid
            // isn't a real time. Re-derive it below.
        }
        // Heal placeholder created_at (the rowid backfill, values << a real epoch-ms):
        // use the EARLIEST link the resource is in — a real timestamp, since you can
        // only link something that already exists. Orphans (no links) have no
        // recoverable time, so they become NULL (exported as null — honest "unknown",
        // not a fake number). `updated_at` is deliberately left as the rowid proxy so
        // the existing list order isn't disturbed (it's an internal sort key, never
        // exported). Idempotent: real ms timestamps (>1e12) are never re-touched, and
        // a NULL orphan stays NULL. New captures stamp real times and skip this entirely.
        const REAL_EPOCH_MS_FLOOR: i64 = 1_000_000_000_000; // ~2001-09; rowids are way below
        conn.execute(
            "UPDATE resources SET
                 created_at = (SELECT MIN(l.created_at) FROM links l WHERE l.lo_id = resources.id OR l.hi_id = resources.id)
             WHERE created_at < ?1",
            params![REAL_EPOCH_MS_FLOOR],
        )?;
        Ok(())
    }

    /// A fresh universally-unique id (UUID v4) for a resource. Generated ONCE on a
    /// resource's first insert and stored; stable thereafter (re-capture keeps it).
    /// The URI remains the primary key — the guid is a stable, portable alias that
    /// the export uses so links can reference resources by guid instead of repeating
    /// the long URI.
    pub fn new_guid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// A resource's stable id (RT-1): its primary URI. Two identities sharing a URI
    /// are the same resource.
    fn id_of(r: &ResourceIdentity) -> &str {
        &r.uri
    }

    /// Insert or update a resource at time `now` (ms). On first insert: set guid +
    /// created_at + updated_at. On re-capture (conflict): bump updated_at (so the
    /// list resurfaces it), keep guid + created_at. `now` is caller-supplied (core
    /// has no clock — same contract as `link`).
    pub fn upsert_resource(&self, r: &ResourceIdentity, now: i64) -> rusqlite::Result<String> {
        let id = Self::id_of(r).to_string();
        // guid: set on first insert; ON CONFLICT does NOT touch it, so re-capture
        // keeps the resource's original UUID stable.
        self.conn.execute(
            "INSERT INTO resources (id, name, uri, path, content_hash, guid, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, path=excluded.path,
                                           content_hash=excluded.content_hash,
                                           updated_at=excluded.updated_at",
            params![id, r.name, r.uri, r.path, r.content_hash, Self::new_guid(), now],
        )?;
        Ok(id)
    }

    /// Link two resources together (bidirectional). Idempotent: linking an
    /// already-linked pair (in either order) is a no-op that returns the existing
    /// link. Atomic via a single transaction (T2).
    pub fn link(
        &mut self,
        a: &ResourceIdentity,
        b: &ResourceIdentity,
        created_at: Option<i64>,
    ) -> rusqlite::Result<Link> {
        let tx = self.conn.transaction()?;
        let a_id = a.uri.clone();
        let b_id = b.uri.clone();
        let now = created_at.unwrap_or(0);
        for r in [a, b] {
            tx.execute(
                "INSERT INTO resources (id, name, uri, path, content_hash, guid, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, path=excluded.path,
                                               content_hash=excluded.content_hash,
                                               updated_at=excluded.updated_at",
                params![r.uri, r.name, r.uri, r.path, r.content_hash, Self::new_guid(), now],
            )?;
        }
        // Canonical ordering so (A,B) and (B,A) collapse to one row.
        let (lo, hi) = if a_id <= b_id {
            (&a_id, &b_id)
        } else {
            (&b_id, &a_id)
        };
        let link_id = format!("{lo}\u{1f}{hi}");
        tx.execute(
            "INSERT INTO links (id, lo_id, hi_id, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(lo_id, hi_id) DO NOTHING",
            params![link_id, lo, hi, created_at],
        )?;
        tx.commit()?;
        Ok(Link {
            id: link_id,
            source_id: a_id,
            target_id: b_id,
            created_at,
        })
    }

    /// Link two resources that ALREADY exist in the graph, by uri (the browser's
    /// "L" action). Bidirectional + idempotent like `link`, but it does NOT upsert
    /// the resources — they're already captured, so their titles are left untouched.
    /// A self-link (a == b) is a no-op.
    pub fn link_existing(
        &self,
        a_uri: &str,
        b_uri: &str,
        created_at: Option<i64>,
    ) -> rusqlite::Result<()> {
        if a_uri == b_uri {
            return Ok(());
        }
        let (lo, hi) = if a_uri <= b_uri {
            (a_uri, b_uri)
        } else {
            (b_uri, a_uri)
        };
        let link_id = format!("{lo}\u{1f}{hi}");
        self.conn.execute(
            "INSERT INTO links (id, lo_id, hi_id, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(lo_id, hi_id) DO NOTHING",
            params![link_id, lo, hi, created_at],
        )?;
        Ok(())
    }

    /// Backlinks (U5): every resource linked to `uri`, found from EITHER endpoint
    /// of the stored row. This is the proof that one stored link is bidirectional.
    pub fn backlinks(&self, uri: &str) -> rusqlite::Result<Vec<ResourceIdentity>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.name, r.uri, r.path, r.content_hash
             FROM links l
             JOIN resources r ON r.id = CASE WHEN l.lo_id = ?1 THEN l.hi_id ELSE l.lo_id END
             WHERE l.lo_id = ?1 OR l.hi_id = ?1
             ORDER BY r.uri",
        )?;
        let rows = stmt.query_map(params![uri], |row| {
            Ok(ResourceIdentity {
                name: row.get(1)?,
                uri: row.get(2)?,
                path: row.get(3)?,
                bookmark: None, // bookmarks are not persisted in the graph (opaque, large)
                content_hash: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Every captured resource, by title (name), A→Z — the link-browser's top level
    /// (Cmd+Shift+L). The browser shows these names, never the raw URIs.
    pub fn list_resources(&self) -> rusqlite::Result<Vec<ResourceIdentity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, uri, path, content_hash FROM resources ORDER BY name COLLATE NOCASE, uri",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ResourceIdentity {
                name: row.get(1)?,
                uri: row.get(2)?,
                path: row.get(3)?,
                bookmark: None,
                content_hash: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Every captured resource with its link count — the browser shows the count
    /// on the right so you can see at a glance what's connected and how richly.
    pub fn list_resources_with_counts(&self) -> rusqlite::Result<Vec<(ResourceIdentity, i64)>> {
        // Newest capture first: rowid increments on first insert and is preserved by
        // the upsert (ON CONFLICT keeps it), so DESC ≈ most-recently-captured on top.
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.name, r.uri, r.path, r.content_hash,
                    (SELECT COUNT(*) FROM links l WHERE l.lo_id = r.id OR l.hi_id = r.id)
             FROM resources r ORDER BY r.updated_at DESC, r.rowid DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                ResourceIdentity {
                    name: row.get(1)?,
                    uri: row.get(2)?,
                    path: row.get(3)?,
                    bookmark: None,
                    content_hash: row.get(4)?,
                },
                row.get(5)?,
            ))
        })?;
        rows.collect()
    }

    /// Backlinks of `uri`, each annotated with ITS OWN total link count (so the
    /// drill-in view shows where you can keep navigating).
    pub fn linked_with_counts(&self, uri: &str) -> rusqlite::Result<Vec<(ResourceIdentity, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.name, r.uri, r.path, r.content_hash,
                    (SELECT COUNT(*) FROM links l2 WHERE l2.lo_id = r.id OR l2.hi_id = r.id)
             FROM links l
             JOIN resources r ON r.id = CASE WHEN l.lo_id = ?1 THEN l.hi_id ELSE l.lo_id END
             WHERE l.lo_id = ?1 OR l.hi_id = ?1
             ORDER BY r.name COLLATE NOCASE, r.uri",
        )?;
        let rows = stmt.query_map(params![uri], |row| {
            Ok((
                ResourceIdentity {
                    name: row.get(1)?,
                    uri: row.get(2)?,
                    path: row.get(3)?,
                    bookmark: None,
                    content_hash: row.get(4)?,
                },
                row.get(5)?,
            ))
        })?;
        rows.collect()
    }

    /// Whether two resources are linked (order-independent).
    pub fn are_linked(&self, a_uri: &str, b_uri: &str) -> rusqlite::Result<bool> {
        let (lo, hi) = if a_uri <= b_uri {
            (a_uri, b_uri)
        } else {
            (b_uri, a_uri)
        };
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM links WHERE lo_id = ?1 AND hi_id = ?2",
            params![lo, hi],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Total number of links (for assertions about dedup/idempotency).
    pub fn link_count(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM links", [], |r| r.get(0))
    }

    /// Number of captured resources (Settings shows this; export sanity).
    pub fn resource_count(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM resources", [], |r| r.get(0))
    }

    /// Wipe the graph — every resource and link (Settings → Reset, destructive).
    /// Returns how many resources were removed. Single transaction.
    pub fn clear(&mut self) -> rusqlite::Result<i64> {
        let n = self.resource_count()?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM links", [])?;
        tx.execute("DELETE FROM resources", [])?;
        tx.commit()?;
        Ok(n)
    }

    /// Import a graph JSON (Settings → Import). MERGES into the current graph:
    /// resources upsert by URI, links re-created idempotently — so importing the
    /// same file twice is a no-op, and importing into a non-empty graph adds to it.
    /// Returns (resources_seen, links_seen). Hand-parsed (no serde dep on the type)
    /// using the same field shape `export_json` writes.
    pub fn import_json(&mut self, json: &str) -> Result<(usize, usize), String> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("not valid JSON: {e}"))?;
        let schema = v.get("schema").and_then(|s| s.as_str()).unwrap_or("");
        // v3 = guid endpoints (current); v2 = indexed; v1 = full-URI (all back-compat).
        enum Ref {
            Guid,
            Index,
            Uri,
        }
        let kind = match schema {
            "ubilink.graph.v3" => Ref::Guid,
            "ubilink.graph.v2" => Ref::Index,
            "ubilink.graph.v1" => Ref::Uri,
            _ => {
                return Err(format!(
                    "unrecognized export schema: '{schema}' (expected ubilink.graph.v3)"
                ))
            }
        };
        let resources = v
            .get("resources")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let links = v
            .get("links")
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();

        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        let mut order: Vec<String> = Vec::with_capacity(resources.len()); // index -> uri (v2)
        let mut by_guid: HashMap<String, String> = HashMap::new(); // guid  -> uri (v3)
        let mut nres = 0usize;
        for r in &resources {
            let uri = r.get("uri").and_then(|x| x.as_str());
            let name = r.get("name").and_then(|x| x.as_str());
            let (uri, name) = match (uri, name) {
                (Some(u), Some(n)) if !u.is_empty() => (u, n),
                _ => {
                    order.push(String::new());
                    continue;
                } // keep index alignment
            };
            let path = r.get("path").and_then(|x| x.as_str());
            // carry the original capture time when present; else 0 (legacy/foreign)
            let created = r.get("created_at").and_then(|x| x.as_i64()).unwrap_or(0);
            // preserve the file's guid (so export→import keeps identity + resolves
            // its links); generate a fresh one for v1/v2 files that carry none.
            let guid = r
                .get("guid")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(Self::new_guid);
            tx.execute(
                "INSERT INTO resources (id, name, uri, path, content_hash, guid, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, path=excluded.path,
                                               updated_at=excluded.updated_at",
                params![uri, name, uri, path, guid, created],
            )
            .map_err(|e| e.to_string())?;
            by_guid.insert(guid, uri.to_string());
            order.push(uri.to_string());
            nres += 1;
        }
        // Resolve a link endpoint to a URI: by guid (v3), index (v2), or literal (v1).
        let endpoint = |val: &serde_json::Value| -> Option<String> {
            match kind {
                Ref::Guid => by_guid.get(val.as_str()?).cloned(),
                Ref::Index => order
                    .get(val.as_u64()? as usize)
                    .filter(|u| !u.is_empty())
                    .cloned(),
                Ref::Uri => val.as_str().map(|s| s.to_string()),
            }
        };
        let mut nlinks = 0usize;
        for l in &links {
            let a = l.get("a").and_then(endpoint);
            let b = l.get("b").and_then(endpoint);
            let (a, b) = match (a, b) {
                (Some(a), Some(b)) if a != b => (a, b),
                _ => continue,
            };
            let (a, b) = (a.as_str(), b.as_str());
            let at = l.get("created_at").and_then(|x| x.as_i64());
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            // only link resources that exist (FK); skip dangling endpoints
            let exists: i64 = tx
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM resources WHERE id=?1) + (SELECT COUNT(*) FROM resources WHERE id=?2)",
                    params![lo, hi],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if exists < 2 {
                continue;
            }
            let link_id = format!("{lo}\u{1f}{hi}");
            tx.execute(
                "INSERT INTO links (id, lo_id, hi_id, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(lo_id, hi_id) DO NOTHING",
                params![link_id, lo, hi, at],
            )
            .map_err(|e| e.to_string())?;
            nlinks += 1;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok((nres, nlinks))
    }

    /// Export the whole graph as JSON (Settings → Export). Resources + links,
    /// hand-serialized (no serde dep on the graph type) so it's stable + readable.
    pub fn export_json(&self) -> rusqlite::Result<String> {
        fn esc(s: &str) -> String {
            let mut o = String::with_capacity(s.len() + 2);
            for c in s.chars() {
                match c {
                    '"' => o.push_str("\\\""),
                    '\\' => o.push_str("\\\\"),
                    '\n' => o.push_str("\\n"),
                    '\r' => o.push_str("\\r"),
                    '\t' => o.push_str("\\t"),
                    c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
                    c => o.push(c),
                }
            }
            o
        }
        // v3: each resource carries its stable `guid` (+ `created_at`); links
        // reference their endpoints BY GUID — robust to reordering/hand-editing,
        // and never repeats the long URI (the URI is the id, so repeating it bloated
        // every link). `path` is emitted only when present (Finder's RT-3 identifier).
        let mut res = Vec::new();
        let mut guid_of: HashMap<String, String> = HashMap::new(); // uri -> guid
        {
            let mut stmt = self.conn.prepare(
                "SELECT name, uri, path, guid, created_at FROM resources ORDER BY updated_at DESC, rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })?;
            for row in rows {
                let (name, uri, path, guid, created_at) = row?;
                let guid = guid.unwrap_or_else(Self::new_guid);
                guid_of.insert(uri.clone(), guid.clone());
                let at = created_at
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "null".into());
                let mut obj = format!(
                    "{{\"guid\":\"{}\",\"name\":\"{}\",\"uri\":\"{}\",\"created_at\":{}",
                    esc(&guid),
                    esc(&name),
                    esc(&uri),
                    at
                );
                if let Some(p) = path {
                    obj.push_str(&format!(",\"path\":\"{}\"", esc(&p)));
                }
                obj.push('}');
                res.push(obj);
            }
        }
        let mut links = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT lo_id, hi_id, created_at FROM links ORDER BY created_at")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?;
            for row in rows {
                let (lo, hi, at) = row?;
                let (Some(a), Some(b)) = (guid_of.get(&lo), guid_of.get(&hi)) else {
                    continue;
                };
                let at = at.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
                links.push(format!(
                    "{{\"a\":\"{}\",\"b\":\"{}\",\"created_at\":{}}}",
                    esc(a),
                    esc(b),
                    at
                ));
            }
        }
        Ok(format!(
            "{{\"schema\":\"ubilink.graph.v3\",\"resources\":[{}],\"links\":[{}]}}",
            res.join(","),
            links.join(",")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, uri: &str) -> ResourceIdentity {
        ResourceIdentity::new(name, uri)
    }

    #[test]
    fn backlink_visible_from_both_ends() {
        // RT-4 / U5: link A<->B once; B is a backlink of A AND A is a backlink of B.
        let mut g = LinkGraph::open_in_memory().unwrap();
        let a = r("Note", "obsidian://Note");
        let b = r("Paper.pdf", "file:///Paper.pdf");
        g.link(&a, &b, Some(1)).unwrap();

        let from_a: Vec<String> = g
            .backlinks("obsidian://Note")
            .unwrap()
            .into_iter()
            .map(|x| x.uri)
            .collect();
        let from_b: Vec<String> = g
            .backlinks("file:///Paper.pdf")
            .unwrap()
            .into_iter()
            .map(|x| x.uri)
            .collect();
        assert_eq!(from_a, vec!["file:///Paper.pdf"]);
        assert_eq!(from_b, vec!["obsidian://Note"]);
    }

    #[test]
    fn link_existing_links_two_captured_resources_without_touching_titles() {
        let g = LinkGraph::open_in_memory().unwrap();
        g.upsert_resource(&r("Note", "obsidian://Note"), 1).unwrap();
        g.upsert_resource(&r("Paper.pdf", "file:///Paper.pdf"), 1)
            .unwrap();
        g.link_existing("obsidian://Note", "file:///Paper.pdf", Some(1))
            .unwrap();
        assert!(g
            .are_linked("obsidian://Note", "file:///Paper.pdf")
            .unwrap());
        // bidirectional + idempotent + no self-links + titles preserved
        g.link_existing("file:///Paper.pdf", "obsidian://Note", Some(2))
            .unwrap();
        g.link_existing("obsidian://Note", "obsidian://Note", Some(3))
            .unwrap(); // self -> no-op
        assert_eq!(g.link_count().unwrap(), 1);
        let titles: Vec<String> = g
            .list_resources()
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert_eq!(
            titles,
            vec!["Note", "Paper.pdf"],
            "linking did not overwrite names"
        );
    }

    #[test]
    fn migration_heals_rowid_placeholder_created_at_from_links() {
        // Simulate a legacy DB: rowid-placeholder created_at on resources, but a real
        // link timestamp exists. The heal (run on every open) should re-derive the
        // resource created_at from the link; an orphan resource becomes NULL.
        let g = LinkGraph::open_in_memory().unwrap();
        // two linked resources + one orphan, all with placeholder created_at (=rowid-ish)
        g.conn.execute("INSERT INTO resources (id,name,uri,guid,created_at,updated_at) VALUES ('x://a','A','x://a','g1',1,1)", []).unwrap();
        g.conn.execute("INSERT INTO resources (id,name,uri,guid,created_at,updated_at) VALUES ('x://b','B','x://b','g2',2,2)", []).unwrap();
        g.conn.execute("INSERT INTO resources (id,name,uri,guid,created_at,updated_at) VALUES ('x://orphan','O','x://orphan','g3',3,3)", []).unwrap();
        g.conn.execute("INSERT INTO links (id,lo_id,hi_id,created_at) VALUES ('k','x://a','x://b',1782400000000)", []).unwrap();
        // re-run the heal (idempotent; same as a fresh open would do)
        LinkGraph::migrate(&g.conn).unwrap();
        let at = |id: &str| -> Option<i64> {
            g.conn
                .query_row(
                    "SELECT created_at FROM resources WHERE id=?1",
                    params![id],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(
            at("x://a"),
            Some(1782400000000),
            "linked resource gets the real link time"
        );
        assert_eq!(at("x://b"), Some(1782400000000));
        assert_eq!(
            at("x://orphan"),
            None,
            "orphan with no link -> NULL (unknown), not a fake rowid"
        );
    }

    #[test]
    fn recapture_resurfaces_to_the_top_and_keeps_its_guid() {
        let g = LinkGraph::open_in_memory().unwrap();
        g.upsert_resource(&r("A", "x://a"), 100).unwrap();
        g.upsert_resource(&r("B", "x://b"), 200).unwrap(); // newer -> top
        let top0 = g.list_resources_with_counts().unwrap()[0].0.uri.clone();
        assert_eq!(top0, "x://b");
        let guid_a_before: String = g
            .conn
            .query_row("SELECT guid FROM resources WHERE id='x://a'", [], |r| {
                r.get(0)
            })
            .unwrap();

        // re-capture A at a later time -> it resurfaces above B, guid unchanged
        g.upsert_resource(&r("A", "x://a"), 300).unwrap();
        let top1 = g.list_resources_with_counts().unwrap()[0].0.uri.clone();
        assert_eq!(top1, "x://a", "re-capture floats A back to the top");
        let guid_a_after: String = g
            .conn
            .query_row("SELECT guid FROM resources WHERE id='x://a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            guid_a_before, guid_a_after,
            "guid is stable across re-capture"
        );
        // created_at preserved (still 100), only updated_at moved
        let created: i64 = g
            .conn
            .query_row(
                "SELECT created_at FROM resources WHERE id='x://a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(created, 100);
    }

    #[test]
    fn counts_track_how_many_links_each_resource_has() {
        let mut g = LinkGraph::open_in_memory().unwrap();
        let a = r("A", "x://a");
        let b = r("B", "x://b");
        let c = r("C", "x://c");
        g.link(&a, &b, Some(1)).unwrap();
        g.link(&a, &c, Some(2)).unwrap(); // A is linked to both B and C
        g.upsert_resource(&r("D", "x://d"), 1).unwrap(); // captured, unlinked
        let counts: std::collections::HashMap<String, i64> = g
            .list_resources_with_counts()
            .unwrap()
            .into_iter()
            .map(|(r, n)| (r.uri, n))
            .collect();
        assert_eq!(counts["x://a"], 2);
        assert_eq!(counts["x://b"], 1);
        assert_eq!(counts["x://d"], 0);
        // drill into A: B and C, each reporting their own count (1 each)
        let drilled: Vec<(String, i64)> = g
            .linked_with_counts("x://a")
            .unwrap()
            .into_iter()
            .map(|(r, n)| (r.uri, n))
            .collect();
        assert_eq!(drilled.len(), 2);
        assert!(drilled.iter().all(|(_, n)| *n == 1));
    }

    #[test]
    fn clear_wipes_everything_and_export_round_trips_the_shape() {
        let mut g = LinkGraph::open_in_memory().unwrap();
        g.link(
            &r("Note", "obsidian://Note"),
            &r("Paper", "file:///P.pdf"),
            Some(7),
        )
        .unwrap();
        g.upsert_resource(&r("Solo \"quoted\"", "x://solo"), 1)
            .unwrap(); // exercise JSON escaping
        assert_eq!(g.resource_count().unwrap(), 3);

        let json = g.export_json().unwrap();
        assert!(json.contains("\"schema\":\"ubilink.graph.v3\""));
        assert!(
            json.contains("Solo \\\"quoted\\\""),
            "names are JSON-escaped"
        );
        assert!(
            json.contains("\"created_at\":7"),
            "link created_at preserved"
        );
        assert_eq!(
            json.matches("\"uri\":").count(),
            3,
            "all 3 resources exported"
        );
        // v3: each resource carries a guid (a real UUID); links reference endpoints
        // BY GUID — the long URI is never repeated in a link.
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let guid_of = |uri: &str| -> String {
            v["resources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["uri"] == uri)
                .unwrap()["guid"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let (gn, gp) = (guid_of("obsidian://Note"), guid_of("file:///P.pdf"));
        assert_eq!(gn.len(), 36, "guid is a UUID");
        let link = &v["links"].as_array().unwrap()[0];
        let (a, b) = (link["a"].as_str().unwrap(), link["b"].as_str().unwrap());
        assert!(
            (a == gn && b == gp) || (a == gp && b == gn),
            "link references the two guids"
        );
        assert!(
            !json.contains("obsidian://Note\",\"b\""),
            "links never repeat the URI"
        );
        // the unlinked "Solo" resource has no path -> the key is omitted entirely
        assert!(
            !json.contains("\"path\":null"),
            "null paths are omitted, not emitted"
        );

        let removed = g.clear().unwrap();
        assert_eq!(removed, 3);
        assert_eq!(g.resource_count().unwrap(), 0);
        assert_eq!(g.link_count().unwrap(), 0);
        // export of an empty graph is still valid, empty arrays
        let empty = g.export_json().unwrap();
        assert!(empty.contains("\"resources\":[]") && empty.contains("\"links\":[]"));

        // import round-trips: a fresh graph fed `json` rebuilds the same shape.
        let mut g2 = LinkGraph::open_in_memory().unwrap();
        let (nr, nl) = g2.import_json(&json).unwrap();
        assert_eq!((nr, nl), (3, 1));
        assert_eq!(g2.resource_count().unwrap(), 3);
        assert_eq!(g2.link_count().unwrap(), 1);
        assert!(g2.are_linked("obsidian://Note", "file:///P.pdf").unwrap());
        // idempotent: importing again adds nothing new (upsert + ON CONFLICT)
        g2.import_json(&json).unwrap();
        assert_eq!(g2.resource_count().unwrap(), 3);
        assert_eq!(g2.link_count().unwrap(), 1);
        // a bad schema is rejected with an explanation, not a silent no-op
        assert!(g2
            .import_json("{\"schema\":\"nope\"}")
            .unwrap_err()
            .contains("schema"));

        // v1 (full-URI links) still imports — back-compat with older exports.
        let mut g3 = LinkGraph::open_in_memory().unwrap();
        let v1 = r#"{"schema":"ubilink.graph.v1","resources":[
            {"name":"X","uri":"x://1","path":null},{"name":"Y","uri":"x://2","path":null}],
            "links":[{"a":"x://1","b":"x://2","created_at":9}]}"#;
        let (r1, l1) = g3.import_json(v1).unwrap();
        assert_eq!((r1, l1), (2, 1));
        assert!(g3.are_linked("x://1", "x://2").unwrap());
    }

    #[test]
    fn list_resources_returns_every_resource_by_title() {
        // The link-browser's top level: every captured resource, sorted by title.
        let mut g = LinkGraph::open_in_memory().unwrap();
        g.link(
            &r("Paper.pdf", "file:///Paper.pdf"),
            &r("Note", "obsidian://Note"),
            Some(1),
        )
        .unwrap();
        g.upsert_resource(&r("Acme", "https://acme.example"), 1)
            .unwrap(); // a standalone, unlinked resource
        let titles: Vec<String> = g
            .list_resources()
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert_eq!(
            titles,
            vec!["Acme", "Note", "Paper.pdf"],
            "A->Z by title, includes unlinked"
        );
    }

    #[test]
    fn link_is_idempotent_in_either_order() {
        let mut g = LinkGraph::open_in_memory().unwrap();
        let a = r("A", "x://a");
        let b = r("B", "x://b");
        g.link(&a, &b, Some(1)).unwrap();
        g.link(&a, &b, Some(2)).unwrap(); // same pair
        g.link(&b, &a, Some(3)).unwrap(); // reversed order
        assert_eq!(
            g.link_count().unwrap(),
            1,
            "A<->B must collapse to ONE link"
        );
        assert!(g.are_linked("x://a", "x://b").unwrap());
        assert!(g.are_linked("x://b", "x://a").unwrap());
    }

    #[test]
    fn persists_across_reopen() {
        // RT-4: the graph is durable. Link, drop the handle, reopen the file.
        let dir = std::env::temp_dir().join(format!("ubilink-graph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("graph.sqlite");
        let p = path.to_string_lossy().to_string();
        {
            let mut g = LinkGraph::open(&p).unwrap();
            g.link(&r("A", "x://a"), &r("B", "x://b"), Some(1)).unwrap();
        }
        {
            let g = LinkGraph::open(&p).unwrap();
            assert!(
                g.are_linked("x://a", "x://b").unwrap(),
                "link did not persist"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_links_to_same_pair_stay_consistent() {
        // GAP-17: many threads racing to link the SAME pair must yield exactly one
        // link and zero errors (WAL + busy_timeout + UNIQUE + INSERT OR IGNORE).
        let dir = std::env::temp_dir().join(format!("ubilink-graph-conc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("graph.sqlite");
        let p = path.to_string_lossy().to_string();
        LinkGraph::open(&p).unwrap(); // create schema first

        let mut handles = vec![];
        for _ in 0..8 {
            let p2 = p.clone();
            handles.push(std::thread::spawn(move || {
                let mut g = LinkGraph::open(&p2).unwrap();
                g.link(&r("A", "x://a"), &r("B", "x://b"), Some(1)).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let g = LinkGraph::open(&p).unwrap();
        assert_eq!(
            g.link_count().unwrap(),
            1,
            "concurrent links duplicated the link"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
