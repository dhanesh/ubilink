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
