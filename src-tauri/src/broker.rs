// Satisfies: RT-5 (binding), RT-6, S1 (invariant), TN1 (capability broker),
// SC-3 (revocable grants).
// See .manifold/linking-manifesto-app.md
//
// The capability broker. Pure Rust, no I/O — unit-testable in isolation and the
// single point where "may this plugin do X?" is decided.
//
// RT-6: a manifest declares the capabilities a plugin needs (`needs[]`); grants
// are per-integration. The broker enforces the grant.
// TN1: the broker mediates EVERY OS/graph access; a plugin sees only what it
// declared and nothing ambient. The link graph is never exposed wholesale.
// S1: deny-by-default — anything not matched by a grant is denied, AND anything
// the broker has no handler for is denied (so "give me the whole graph" can never
// be served, even if a manifest somehow asked for it).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

/// An integration manifest (RT-6). Mirrors schema/integration.manifest.schema.json.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub app: String,
    pub version: String,
    /// Capability strings the plugin declares it needs, e.g.
    /// "open:obsidian://" or "read:focused-resource".
    #[serde(default)]
    pub needs: Vec<String>,
    /// Placeholder for the bundle signature (S2/provenance, future task).
    #[serde(default)]
    pub signature: Option<String>,
}

impl Manifest {
    pub fn from_path(path: &Path) -> io::Result<Manifest> {
        let raw = fs::read_to_string(path)?;
        let m: Manifest = serde_json::from_str(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        m.validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(m)
    }

    /// Structural validation (RT-6). A full JSON-Schema validation against
    /// schema/integration.manifest.schema.json could be wired with the
    /// `jsonschema` crate later; we keep deps minimal here and check the
    /// invariants the broker relies on.
    pub fn validate(&self) -> Result<(), String> {
        for (field, val) in [
            ("id", &self.id),
            ("name", &self.name),
            ("app", &self.app),
            ("version", &self.version),
        ] {
            if val.trim().is_empty() {
                return Err(format!("manifest field `{field}` must be non-empty"));
            }
        }
        for need in &self.needs {
            Capability::parse(need)
                .ok_or_else(|| format!("invalid capability string in needs[]: {need:?}"))?;
        }
        Ok(())
    }
}

/// A capability string parsed into `verb:scope`, e.g. `open` + `obsidian://`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub verb: String,
    pub scope: String,
}

impl Capability {
    pub fn parse(s: &str) -> Option<Capability> {
        let (verb, scope) = s.split_once(':')?;
        if verb.is_empty() || scope.is_empty() {
            return None;
        }
        Some(Capability {
            verb: verb.to_string(),
            scope: scope.to_string(),
        })
    }

    /// Does THIS (a grant) authorize `request`?
    /// - verbs must be identical
    /// - a scheme-prefix grant (scope ends with `://`) authorizes any request
    ///   whose scope starts with it, e.g. grant `open:obsidian://` authorizes
    ///   `open:obsidian://vault/Note.md`.
    /// - otherwise scopes must be exactly equal.
    /// Note: there is intentionally NO wildcard semantics. A grant of
    /// `read:*` would only ever match a literal request scope `*`, never
    /// `link-graph` — so no grant string can smuggle wholesale access.
    fn authorizes(&self, request: &Capability) -> bool {
        if self.verb != request.verb {
            return false;
        }
        if self.scope.ends_with("://") {
            request.scope.starts_with(&self.scope)
        } else {
            self.scope == request.scope
        }
    }
}

/// The single resource a plugin was invoked for. This is the ONLY graph-adjacent
/// data a plugin can ever read (S1: never the link graph wholesale).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusedResource {
    pub id: String,
    pub name: String,
    pub uri: String,
}

/// The broker's answer to a capability request.
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub allowed: bool,
    pub result: Option<Value>,
    pub reason: String,
}

impl Decision {
    fn allow(result: Value, reason: &str) -> Decision {
        Decision { allowed: true, result: Some(result), reason: reason.to_string() }
    }
    fn deny(reason: &str) -> Decision {
        Decision { allowed: false, result: None, reason: reason.to_string() }
    }
}

/// Per-invocation capability broker. Holds the grant for ONE plugin and the ONE
/// resource it is invoked for. Grants can be revoked mid-session (SC-3).
pub struct Broker {
    grants: Vec<String>,
    revoked: HashSet<String>,
    resource: FocusedResource,
}

impl Broker {
    pub fn new(manifest: &Manifest, resource: FocusedResource) -> Broker {
        Broker {
            grants: manifest.needs.clone(),
            revoked: HashSet::new(),
            resource,
        }
    }

    /// SC-3: force-invalidate a previously-issued grant. Idempotent. After this,
    /// any request that would have matched `grant` is denied.
    pub fn revoke(&mut self, grant: &str) {
        self.revoked.insert(grant.to_string());
    }

    /// The grant string (if any) that authorizes `request` and is not revoked.
    fn matching_grant(&self, request: &Capability) -> Option<&String> {
        self.grants.iter().find(|g| {
            Capability::parse(g)
                .map(|cap| cap.authorizes(request) && !self.revoked.contains(*g))
                .unwrap_or(false)
        })
    }

    /// Decide a capability request. This is the chokepoint TN1 requires every
    /// OS/graph access to pass through.
    pub fn decide(&self, request_str: &str) -> Decision {
        let request = match Capability::parse(request_str) {
            Some(c) => c,
            None => return Decision::deny("malformed capability string"),
        };

        // Deny-by-default: must be authorized by a (non-revoked) grant.
        if self.matching_grant(&request).is_none() {
            // Distinguish a revoked grant for a clearer audit trail (SC-3).
            let revoked_match = self.grants.iter().any(|g| {
                Capability::parse(g)
                    .map(|cap| cap.authorizes(&request) && self.revoked.contains(g))
                    .unwrap_or(false)
            });
            return if revoked_match {
                Decision::deny("grant revoked (SC-3)")
            } else {
                Decision::deny("deny-by-default: capability not in manifest grant")
            };
        }

        // Authorized — now dispatch to a broker handler. Only explicitly
        // implemented privileged actions are served. There is NO handler that
        // returns the link graph wholesale, so even an authorized but unknown
        // capability is denied (S1 belt-and-suspenders).
        match request.verb.as_str() {
            "open" => Decision::allow(
                json!({ "opened": request.scope }),
                "broker performed the mediated open (the plugin never touches the OS directly)",
            ),
            "read" if request.scope == "focused-resource" => Decision::allow(
                json!(self.resource),
                "broker returned ONLY the single focused resource (never the link graph)",
            ),
            _ => Decision::deny(
                "no broker handler: capability cannot be served (e.g. wholesale link-graph access is never implemented)",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res() -> FocusedResource {
        FocusedResource {
            id: "r1".into(),
            name: "Note.md".into(),
            uri: "obsidian://vault/Note.md".into(),
        }
    }

    fn manifest(needs: &[&str]) -> Manifest {
        Manifest {
            id: "x".into(),
            name: "x".into(),
            app: "x".into(),
            version: "1.0.0".into(),
            needs: needs.iter().map(|s| s.to_string()).collect(),
            signature: None,
        }
    }

    #[test]
    fn grant_allows_scheme_prefix() {
        let b = Broker::new(&manifest(&["open:obsidian://"]), res());
        let d = b.decide("open:obsidian://vault/Note.md");
        assert!(d.allowed, "{}", d.reason);
    }

    #[test]
    fn deny_by_default_when_not_granted() {
        let b = Broker::new(&manifest(&["open:obsidian://"]), res());
        assert!(!b.decide("read:link-graph").allowed);
        assert!(!b.decide("open:evil://x").allowed);
        assert!(!b.decide("read:focused-resource").allowed);
    }

    #[test]
    fn no_wholesale_graph_even_if_manifest_asks() {
        // Adversarial manifest tries every phrasing of "give me the graph".
        let b = Broker::new(
            &manifest(&["read:link-graph", "read:graph", "read:*", "read:all"]),
            res(),
        );
        for req in ["read:link-graph", "read:graph", "read:*", "read:all", "read:everything"] {
            let d = b.decide(req);
            assert!(!d.allowed, "graph leaked via {req}: {}", d.reason);
        }
        // The only read that works is the single focused resource.
        let b2 = Broker::new(&manifest(&["read:focused-resource"]), res());
        let d = b2.decide("read:focused-resource");
        assert!(d.allowed);
        assert_eq!(d.result.unwrap()["id"], "r1");
    }

    #[test]
    fn revoke_flips_allow_to_deny() {
        let mut b = Broker::new(&manifest(&["open:obsidian://"]), res());
        assert!(b.decide("open:obsidian://vault/Note.md").allowed);
        b.revoke("open:obsidian://");
        let d = b.decide("open:obsidian://vault/Note.md");
        assert!(!d.allowed);
        assert_eq!(d.reason, "grant revoked (SC-3)");
    }
}
