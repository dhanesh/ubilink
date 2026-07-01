// Satisfies: RT-1 (stable link addr+name), RT-3 (multi-identifier identity),
// T3 (best-effort re-resolution), TN5 (confidence-gated re-resolution), SC-2
// (fuzzy match -> confirm).
// See .manifold/linking-manifesto-app.md
//
// The resource-identity model. A "resource" is whatever a link points at: a file,
// a note, an email, a web page. RT-1 requires a stable link carrying an *address*
// and a *name*. RT-3 requires that identity be representable by MULTIPLE
// independently-resolvable identifiers so a link survives the resource moving or
// being renamed (T3): primary URI, filesystem path, an OS security-scoped bookmark
// (opaque bytes), and a content hash. When the primary identifier breaks we
// re-resolve from the others.
//
// TN5 resolution: re-resolution is CONFIDENCE-GATED. An exact match (content hash
// or OS bookmark) auto-resolves silently; a fuzzy match (name/path only) must be
// CONFIRMED by the user before the link is retargeted (SC-2); no match fails
// honestly (the link is left broken-but-recorded, never silently wrong).

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A resource's identity — the bundle of identifiers a link stores (RT-3).
/// `uri` + `name` together are the manifesto's "address and name" (RT-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceIdentity {
    /// Human-facing name (RT-1: the "name" half of a link).
    pub name: String,
    /// Primary address (RT-1: the "address" half), e.g. `obsidian://vault/Note.md`
    /// or `file:///Users/x/a.pdf`. The first identifier we try to resolve.
    pub uri: String,
    /// Filesystem path, when the resource is a file. Survives URI-scheme changes.
    pub path: Option<String>,
    /// Opaque OS security-scoped bookmark bytes (macOS `URL.bookmarkData`, etc.).
    /// Survives a file move/rename within a volume — the strongest non-content id.
    pub bookmark: Option<Vec<u8>>,
    /// Hash of the resource's content. Survives a move AND a rename; identifies the
    /// resource by what it *is*. (Spike uses a std hash; production uses SHA-256.)
    pub content_hash: Option<String>,
}

impl ResourceIdentity {
    /// Minimal identity from just the manifesto-required pair (address + name).
    pub fn new(name: impl Into<String>, uri: impl Into<String>) -> ResourceIdentity {
        ResourceIdentity {
            name: name.into(),
            uri: uri.into(),
            path: None,
            bookmark: None,
            content_hash: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> ResourceIdentity {
        self.path = Some(path.into());
        self
    }

    pub fn with_bookmark(mut self, bookmark: Vec<u8>) -> ResourceIdentity {
        self.bookmark = Some(bookmark);
        self
    }

    /// Set the content hash from the resource's raw bytes. Production should use a
    /// cryptographic hash (SHA-256); the spike uses the std hasher so the crate
    /// stays dependency-light. Only equality matters for re-resolution.
    pub fn with_content(mut self, bytes: &[u8]) -> ResourceIdentity {
        self.content_hash = Some(hash_bytes(bytes));
        self
    }

    /// How many independent identifiers this identity carries (RT-3). A robust
    /// link wants >= 2 so it can survive the primary breaking.
    pub fn identifier_count(&self) -> usize {
        1 // uri is always present
            + self.path.is_some() as usize
            + self.bookmark.is_some() as usize
            + self.content_hash.is_some() as usize
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// A bidirectional link ("link") between two resources. Stored once; the graph
/// (RT-4) makes it queryable from BOTH endpoints. IDs reference resources in the
/// graph; `created_at` is supplied by the caller (the core has no clock here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub created_at: Option<i64>,
}

/// Confidence that a candidate resource IS the one a stored identity referred to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchConfidence {
    /// Content hash or OS bookmark matched — provably the same resource.
    Exact,
    /// Only name/path heuristics matched — probably, but not certainly.
    Fuzzy,
    /// Nothing matched.
    None,
}

/// What re-resolution should DO, given the confidence (TN5 / SC-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionAction {
    /// Retarget the link silently — we are certain (TN5: exact only).
    AutoResolve,
    /// Ask the user "now points to X (moved from Y) — confirm?" before retargeting.
    Confirm,
    /// Leave the link broken-but-recorded; never silently wrong.
    Fail,
}

/// Re-resolve a stored identity against a candidate resource (T3). Returns the
/// match confidence and the gated action TN5 mandates.
pub fn reresolve(
    stored: &ResourceIdentity,
    candidate: &ResourceIdentity,
) -> (MatchConfidence, ResolutionAction) {
    let confidence = match_confidence(stored, candidate);
    let action = match confidence {
        MatchConfidence::Exact => ResolutionAction::AutoResolve,
        MatchConfidence::Fuzzy => ResolutionAction::Confirm,
        MatchConfidence::None => ResolutionAction::Fail,
    };
    (confidence, action)
}

fn match_confidence(stored: &ResourceIdentity, candidate: &ResourceIdentity) -> MatchConfidence {
    // Exact: a shared content hash or a shared bookmark proves identity even if
    // the name AND path changed.
    if both_present_and_equal(&stored.content_hash, &candidate.content_hash)
        || both_present_and_equal(&stored.bookmark, &candidate.bookmark)
    {
        return MatchConfidence::Exact;
    }
    // Fuzzy: heuristic agreement on name or path. Not enough to auto-retarget.
    if stored.name == candidate.name || both_present_and_equal(&stored.path, &candidate.path) {
        return MatchConfidence::Fuzzy;
    }
    MatchConfidence::None
}

fn both_present_and_equal<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_carries_multiple_independent_identifiers() {
        // RT-3: a robust link stores several identifiers, not just the URI.
        let id = ResourceIdentity::new("Note.md", "obsidian://vault/Note.md")
            .with_path("/Users/x/vault/Note.md")
            .with_content(b"hello world");
        assert_eq!(id.identifier_count(), 3);
        assert!(id.content_hash.is_some());
    }

    #[test]
    fn content_hash_matches_after_move_and_rename() {
        // Exact match by content survives BOTH a renamed file and a new path.
        let stored = ResourceIdentity::new("Old.md", "file:///old/Old.md")
            .with_path("/old/Old.md")
            .with_content(b"the same bytes");
        let moved = ResourceIdentity::new("Renamed.md", "file:///new/Renamed.md")
            .with_path("/new/Renamed.md")
            .with_content(b"the same bytes");
        assert_eq!(
            reresolve(&stored, &moved),
            (MatchConfidence::Exact, ResolutionAction::AutoResolve)
        );
    }

    #[test]
    fn bookmark_match_is_exact_auto_resolve() {
        let stored = ResourceIdentity::new("A", "file:///a").with_bookmark(vec![1, 2, 3]);
        let moved = ResourceIdentity::new("B-renamed", "file:///b").with_bookmark(vec![1, 2, 3]);
        assert_eq!(reresolve(&stored, &moved).1, ResolutionAction::AutoResolve);
    }

    #[test]
    fn name_only_match_requires_confirmation() {
        // TN5 / SC-2: same name but DIFFERENT content -> fuzzy -> must confirm,
        // never silently retarget (could be the wrong file).
        let stored = ResourceIdentity::new("Report.md", "file:///old/Report.md")
            .with_content(b"version one");
        let candidate = ResourceIdentity::new("Report.md", "file:///other/Report.md")
            .with_content(b"a totally different document");
        assert_eq!(
            reresolve(&stored, &candidate),
            (MatchConfidence::Fuzzy, ResolutionAction::Confirm)
        );
    }

    #[test]
    fn no_match_fails_rather_than_guessing() {
        let stored = ResourceIdentity::new("A.md", "file:///a/A.md").with_content(b"aaa");
        let unrelated = ResourceIdentity::new("Z.md", "file:///z/Z.md").with_content(b"zzz");
        assert_eq!(
            reresolve(&stored, &unrelated),
            (MatchConfidence::None, ResolutionAction::Fail)
        );
    }

    #[test]
    fn exact_beats_fuzzy_when_both_could_apply() {
        // Same name AND same content -> Exact (content), not merely Fuzzy.
        let stored = ResourceIdentity::new("Same.md", "file:///old").with_content(b"xyz");
        let candidate = ResourceIdentity::new("Same.md", "file:///new").with_content(b"xyz");
        assert_eq!(reresolve(&stored, &candidate).0, MatchConfidence::Exact);
    }
}
