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
                content_hash TEXT
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
        Ok(LinkGraph { conn })
    }

    /// A resource's stable id (RT-1): its primary URI. Two identities sharing a URI
    /// are the same resource.
    fn id_of(r: &ResourceIdentity) -> &str {
        &r.uri
    }

    /// Insert or update a resource, returning its id.
    pub fn upsert_resource(&self, r: &ResourceIdentity) -> rusqlite::Result<String> {
        let id = Self::id_of(r).to_string();
        self.conn.execute(
            "INSERT INTO resources (id, name, uri, path, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, path=excluded.path,
                                           content_hash=excluded.content_hash",
            params![id, r.name, r.uri, r.path, r.content_hash],
        )?;
        Ok(id)
    }

    /// Link two resources together (bidirectional). Idempotent: linking an
    /// already-linked pair (in either order) is a no-op that returns the existing
    /// link. Atomic via a single transaction (T2).
    pub fn link(&mut self, a: &ResourceIdentity, b: &ResourceIdentity, created_at: Option<i64>) -> rusqlite::Result<Link> {
        let tx = self.conn.transaction()?;
        let a_id = a.uri.clone();
        let b_id = b.uri.clone();
        for r in [a, b] {
            tx.execute(
                "INSERT INTO resources (id, name, uri, path, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, path=excluded.path,
                                               content_hash=excluded.content_hash",
                params![r.uri, r.name, r.uri, r.path, r.content_hash],
            )?;
        }
        // Canonical ordering so (A,B) and (B,A) collapse to one row.
        let (lo, hi) = if a_id <= b_id { (&a_id, &b_id) } else { (&b_id, &a_id) };
        let link_id = format!("{lo}\u{1f}{hi}");
        tx.execute(
            "INSERT INTO links (id, lo_id, hi_id, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(lo_id, hi_id) DO NOTHING",
            params![link_id, lo, hi, created_at],
        )?;
        tx.commit()?;
        Ok(Link { id: link_id, source_id: a_id, target_id: b_id, created_at })
    }

    /// Link two resources that ALREADY exist in the graph, by uri (the browser's
    /// "L" action). Bidirectional + idempotent like `link`, but it does NOT upsert
    /// the resources — they're already captured, so their titles are left untouched.
    /// A self-link (a == b) is a no-op.
    pub fn link_existing(&self, a_uri: &str, b_uri: &str, created_at: Option<i64>) -> rusqlite::Result<()> {
        if a_uri == b_uri {
            return Ok(());
        }
        let (lo, hi) = if a_uri <= b_uri { (a_uri, b_uri) } else { (b_uri, a_uri) };
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
             FROM resources r ORDER BY r.rowid DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                ResourceIdentity { name: row.get(1)?, uri: row.get(2)?, path: row.get(3)?, bookmark: None, content_hash: row.get(4)? },
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
                ResourceIdentity { name: row.get(1)?, uri: row.get(2)?, path: row.get(3)?, bookmark: None, content_hash: row.get(4)? },
                row.get(5)?,
            ))
        })?;
        rows.collect()
    }

    /// Whether two resources are linked (order-independent).
    pub fn are_linked(&self, a_uri: &str, b_uri: &str) -> rusqlite::Result<bool> {
        let (lo, hi) = if a_uri <= b_uri { (a_uri, b_uri) } else { (b_uri, a_uri) };
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM links WHERE lo_id = ?1 AND hi_id = ?2",
            params![lo, hi],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Total number of links (for assertions about dedup/idempotency).
    pub fn link_count(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM links", [], |r| r.get(0))
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

        let from_a: Vec<String> = g.backlinks("obsidian://Note").unwrap().into_iter().map(|x| x.uri).collect();
        let from_b: Vec<String> = g.backlinks("file:///Paper.pdf").unwrap().into_iter().map(|x| x.uri).collect();
        assert_eq!(from_a, vec!["file:///Paper.pdf"]);
        assert_eq!(from_b, vec!["obsidian://Note"]);
    }

    #[test]
    fn link_existing_links_two_captured_resources_without_touching_titles() {
        let g = LinkGraph::open_in_memory().unwrap();
        g.upsert_resource(&r("Note", "obsidian://Note")).unwrap();
        g.upsert_resource(&r("Paper.pdf", "file:///Paper.pdf")).unwrap();
        g.link_existing("obsidian://Note", "file:///Paper.pdf", Some(1)).unwrap();
        assert!(g.are_linked("obsidian://Note", "file:///Paper.pdf").unwrap());
        // bidirectional + idempotent + no self-links + titles preserved
        g.link_existing("file:///Paper.pdf", "obsidian://Note", Some(2)).unwrap();
        g.link_existing("obsidian://Note", "obsidian://Note", Some(3)).unwrap(); // self -> no-op
        assert_eq!(g.link_count().unwrap(), 1);
        let titles: Vec<String> = g.list_resources().unwrap().into_iter().map(|x| x.name).collect();
        assert_eq!(titles, vec!["Note", "Paper.pdf"], "linking did not overwrite names");
    }

    #[test]
    fn counts_track_how_many_links_each_resource_has() {
        let mut g = LinkGraph::open_in_memory().unwrap();
        let a = r("A", "x://a");
        let b = r("B", "x://b");
        let c = r("C", "x://c");
        g.link(&a, &b, Some(1)).unwrap();
        g.link(&a, &c, Some(2)).unwrap(); // A is linked to both B and C
        g.upsert_resource(&r("D", "x://d")).unwrap(); // captured, unlinked
        let counts: std::collections::HashMap<String, i64> =
            g.list_resources_with_counts().unwrap().into_iter().map(|(r, n)| (r.uri, n)).collect();
        assert_eq!(counts["x://a"], 2);
        assert_eq!(counts["x://b"], 1);
        assert_eq!(counts["x://d"], 0);
        // drill into A: B and C, each reporting their own count (1 each)
        let drilled: Vec<(String, i64)> = g.linked_with_counts("x://a").unwrap().into_iter().map(|(r, n)| (r.uri, n)).collect();
        assert_eq!(drilled.len(), 2);
        assert!(drilled.iter().all(|(_, n)| *n == 1));
    }

    #[test]
    fn list_resources_returns_every_resource_by_title() {
        // The link-browser's top level: every captured resource, sorted by title.
        let mut g = LinkGraph::open_in_memory().unwrap();
        g.link(&r("Paper.pdf", "file:///Paper.pdf"), &r("Note", "obsidian://Note"), Some(1)).unwrap();
        g.upsert_resource(&r("Acme", "https://acme.example")).unwrap(); // a standalone, unlinked resource
        let titles: Vec<String> = g.list_resources().unwrap().into_iter().map(|x| x.name).collect();
        assert_eq!(titles, vec!["Acme", "Note", "Paper.pdf"], "A->Z by title, includes unlinked");
    }

    #[test]
    fn link_is_idempotent_in_either_order() {
        let mut g = LinkGraph::open_in_memory().unwrap();
        let a = r("A", "x://a");
        let b = r("B", "x://b");
        g.link(&a, &b, Some(1)).unwrap();
        g.link(&a, &b, Some(2)).unwrap(); // same pair
        g.link(&b, &a, Some(3)).unwrap(); // reversed order
        assert_eq!(g.link_count().unwrap(), 1, "A<->B must collapse to ONE link");
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
            assert!(g.are_linked("x://a", "x://b").unwrap(), "link did not persist");
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
        assert_eq!(g.link_count().unwrap(), 1, "concurrent links duplicated the link");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
