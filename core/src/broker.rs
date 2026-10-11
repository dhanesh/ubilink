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

/// An integration manifest (RT-6). Mirrors schema/integration.manifest.schema.json:
/// `deny_unknown_fields` is the schema's `additionalProperties: false` (an unknown
/// field would not be covered by the bundle signature, which signs only the fields
/// below), and `needs` carries no serde default because the schema requires it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub app: String,
    /// Human-facing semantic version (the schema's `version`). The monotonic,
    /// downgrade-protected release number is the integer `WireBundle.version`.
    pub version: String,
    /// Capability strings the plugin declares it needs, e.g.
    /// "open:obsidian://" or "read:focused-resource".
    pub needs: Vec<String>,
    /// Placeholder for the bundle signature (S2/provenance, future task). Always
    /// null in practice: the real signature lives in the wire envelope
    /// (`WireBundle.signature_hex`) and is not part of the signed content.
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

    /// Enforce schema/integration.manifest.schema.json (RT-6) without a JSON-Schema
    /// crate (S2: no new deps). Each check below is the hand-written equivalent of
    /// one schema keyword; keep them in step with the schema file. Required fields
    /// and `additionalProperties: false` are enforced by serde at parse time.
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
        if !is_reverse_dns_id(&self.id) {
            return Err(format!(
                "manifest field `id` must be a lowercase reverse-DNS id like \
                 com.example.app (schema ^[a-z0-9]+(\\.[a-z0-9-]+)+$), got {:?}",
                self.id
            ));
        }
        if !is_semver(&self.version) {
            return Err(format!(
                "manifest field `version` must be a semantic version like 1.0.0 \
                 (schema ^\\d+\\.\\d+\\.\\d+(?:[-+].+)?$), got {:?}",
                self.version
            ));
        }
        let mut seen = HashSet::new();
        for need in &self.needs {
            // (is_capability_string implies Capability::parse succeeds.)
            if !is_capability_string(need) {
                return Err(format!(
                    "invalid capability string in needs[]: {need:?} (expected \
                     `<verb>:<scope>` with a lowercase verb, schema ^[a-z][a-z0-9-]*:.+$)"
                ));
            }
            if !seen.insert(need.as_str()) {
                return Err(format!("duplicate capability in needs[]: {need:?}"));
            }
        }
        Ok(())
    }
}

/// schema `id`: `^[a-z0-9]+(\.[a-z0-9-]+)+$` (minLength 3 is implied: >= "a.b").
fn is_reverse_dns_id(id: &str) -> bool {
    let mut labels = id.split('.');
    let first = labels.next().unwrap_or("");
    let rest: Vec<&str> = labels.collect();
    let label_ok = |l: &str, extra: &[u8]| {
        !l.is_empty()
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || extra.contains(&b))
    };
    label_ok(first, b"") && !rest.is_empty() && rest.iter().all(|l| label_ok(l, b"-"))
}

/// schema `version`: `^\d+\.\d+\.\d+(?:[-+].+)?$`. `\d` is ASCII-only in the
/// schema's ECMA-262 regex dialect, and `.` excludes line terminators.
fn is_semver(v: &str) -> bool {
    let core_end = v.find(['-', '+']).unwrap_or(v.len());
    let (core, suffix) = v.split_at(core_end);
    let parts: Vec<&str> = core.split('.').collect();
    let core_ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    // suffix is empty, or `-`/`+` followed by at least one non-line-terminator.
    let suffix_ok = suffix.is_empty() || (suffix.len() > 1 && !has_line_terminator(suffix));
    core_ok && suffix_ok
}

/// schema `needs[]` item: `^[a-z][a-z0-9-]*:.+$`.
fn is_capability_string(s: &str) -> bool {
    let Some((verb, scope)) = s.split_once(':') else {
        return false;
    };
    let mut vb = verb.bytes();
    let verb_ok = vb.next().is_some_and(|b| b.is_ascii_lowercase())
        && vb.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    verb_ok && !scope.is_empty() && !has_line_terminator(scope)
}

/// ECMA-262 line terminators — the characters a regex `.` does not match.
fn has_line_terminator(s: &str) -> bool {
    s.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
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
    ///
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
        Decision {
            allowed: true,
            result: Some(result),
            reason: reason.to_string(),
        }
    }
    fn deny(reason: &str) -> Decision {
        Decision {
            allowed: false,
            result: None,
            reason: reason.to_string(),
        }
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
            id: "com.example.x".into(),
            name: "x".into(),
            app: "x".into(),
            version: "1.0.0".into(),
            needs: needs.iter().map(|s| s.to_string()).collect(),
            signature: None,
        }
    }

    #[test]
    fn manifest_validate_is_the_author_facing_gate() {
        // B3: an author validates a manifest before signing. A well-formed one
        // passes; empty required fields and malformed capability strings are caught
        // with a message (so authoring fails loudly, not silently downstream).
        assert!(manifest(&["open:obsidian://", "read:focused-resource"])
            .validate()
            .is_ok());

        let mut empty_id = manifest(&[]);
        empty_id.id = "  ".into();
        assert!(empty_id.validate().unwrap_err().contains("id"));

        // a capability string with no `verb:scope` shape is rejected.
        assert!(manifest(&["not-a-capability"]).validate().is_err());
    }

    // ── schema/integration.manifest.schema.json, enforced (not just non-empty) ──

    #[test]
    fn validate_enforces_reverse_dns_id() {
        // schema: ^[a-z0-9]+(\.[a-z0-9-]+)+$, minLength 3
        for good in [
            "com.example.x",
            "example.hang",
            "a.b",
            "com.ex-ample.v2",
            "0.1",
        ] {
            let mut m = manifest(&[]);
            m.id = good.into();
            assert!(m.validate().is_ok(), "{good:?} should be a valid id");
        }
        for bad in [
            "x",             // no dot
            "Com.Example.X", // uppercase
            "com..example",  // empty label
            "com.example.",  // trailing dot
            ".com.example",  // leading dot
            "-com.example",  // first label may not contain '-'
            "com.exa_mple",  // underscore
            "com.example/x", // path char
            "com.example x", // space
            "com.example\n", // line terminator
            "com.exämple",   // non-ASCII
        ] {
            let mut m = manifest(&[]);
            m.id = bad.into();
            let err = m
                .validate()
                .expect_err(&format!("{bad:?} must be rejected"));
            assert!(err.contains("id"), "error should name the field: {err}");
        }
    }

    #[test]
    fn validate_enforces_semver_version() {
        // schema: ^\d+\.\d+\.\d+(?:[-+].+)?$
        for good in [
            "1.0.0",
            "0.0.1",
            "10.20.30",
            "1.0.0-rc.1",
            "1.0.0+build.5",
            "6.6.6",
        ] {
            let mut m = manifest(&[]);
            m.version = good.into();
            assert!(m.validate().is_ok(), "{good:?} should be a valid version");
        }
        for bad in [
            "1", "1.0", "v1.0.0", "1.0.0.0", "1.0.0-", "1.0.0+", "1.a.0", "1.0.0 ", " 1.0.0",
            "1.0.0-\n", "١.٠.٠", // non-ASCII digits are not \d in the schema's (ECMA) regex
        ] {
            let mut m = manifest(&[]);
            m.version = bad.into();
            let err = m
                .validate()
                .expect_err(&format!("{bad:?} must be rejected"));
            assert!(
                err.contains("version"),
                "error should name the field: {err}"
            );
        }
    }

    #[test]
    fn validate_enforces_lowercase_capability_verb() {
        // schema: needs[] items ^[a-z][a-z0-9-]*:.+$
        assert!(manifest(&["open:obsidian://", "read-v2:focused-resource"])
            .validate()
            .is_ok());
        for bad in [
            "Open:obsidian://",
            "OPEN:obsidian://",
            "1open:obsidian://",
            "-open:x",
            "op en:x",
            "open:",
            ":x",
            "open:line\nbreak",
        ] {
            let err = manifest(&[bad])
                .validate()
                .expect_err(&format!("{bad:?} must be rejected"));
            assert!(err.contains("needs"), "error should name the field: {err}");
        }
    }

    #[test]
    fn validate_rejects_duplicate_needs() {
        // schema: needs uniqueItems
        let err = manifest(&["open:obsidian://", "open:obsidian://"])
            .validate()
            .unwrap_err();
        assert!(err.contains("needs"), "{err}");
    }

    #[test]
    fn deserialize_rejects_unknown_fields_and_missing_needs() {
        // schema: additionalProperties false. An unknown field is not covered by the
        // signature (canonical_message signs only the known fields), so accepting it
        // would let a mirror smuggle unsigned data into a "verified" manifest.
        let base = r#""id":"com.example.x","name":"X","app":"Obsidian","version":"1.0.0""#;
        let ok = format!(r#"{{{base},"needs":[],"signature":null}}"#);
        assert!(serde_json::from_str::<Manifest>(&ok).is_ok());
        let no_sig = format!(r#"{{{base},"needs":[]}}"#);
        assert!(serde_json::from_str::<Manifest>(&no_sig).is_ok());

        let unknown = format!(r#"{{{base},"needs":[],"permissions":["read:link-graph"]}}"#);
        let err = serde_json::from_str::<Manifest>(&unknown)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"), "{err}");

        // schema: `needs` is required.
        let missing = format!("{{{base}}}");
        assert!(serde_json::from_str::<Manifest>(&missing).is_err());
    }

    #[test]
    fn every_repo_manifest_passes_the_schema_rules() {
        // The example manifests (and the one release.yml signs: examples/benign)
        // must keep loading under the stricter rules.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/examples");
        let mut seen = 0;
        for entry in std::fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path().join("manifest.json");
            if path.exists() {
                Manifest::from_path(&path)
                    .unwrap_or_else(|e| panic!("{} no longer loads: {e}", path.display()));
                seen += 1;
            }
        }
        assert!(seen >= 4, "expected the example manifests, found {seen}");
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
        for req in [
            "read:link-graph",
            "read:graph",
            "read:*",
            "read:all",
            "read:everything",
        ] {
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

    #[test]
    fn revoked_focused_resource_is_no_longer_served() {
        // The broker is the ONLY channel the focused resource reaches a plugin
        // through (the sandbox never pushes it), so revocation must stop it here.
        let mut b = Broker::new(&manifest(&["read:focused-resource"]), res());
        assert_eq!(
            b.decide("read:focused-resource").result.unwrap()["id"],
            "r1"
        );
        b.revoke("read:focused-resource");
        let d = b.decide("read:focused-resource");
        assert!(!d.allowed);
        assert!(
            d.result.is_none(),
            "revoked read still carried the resource"
        );
        assert_eq!(d.reason, "grant revoked (SC-3)");
    }
}
