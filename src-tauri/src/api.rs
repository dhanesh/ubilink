// Satisfies: B1 (the API half — obtain/construct a link, the manifesto's 2nd ask),
// S5 (the local link API is bounded + rate-limited), SC-1 (input validation:
// scheme allowlist, payload size cap, encoding normalization, validation),
// GAP-10 (resource exhaustion on the unauthenticated local surface).
// See .manifold/linking-manifesto-app.md
//
// The local link API. The Linking Manifesto's second ask is "an API to obtain or
// construct a link (address + name)". That API is a local, unauthenticated
// surface, so S5/GAP-10 require it to be bounded: input size capped, a scheme
// allowlist (no `javascript:`/`data:` smuggling), encoding normalized, and the
// whole thing rate-limited so a local process can't exhaust resources.

use crate::resource::ResourceIdentity;
use std::time::Instant;

/// Max accepted size of any single field (S5: bound input). 64 KiB.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;

/// Default scheme allowlist (SC-1). Extend via `LinkApi::with_schemes`.
const DEFAULT_SCHEMES: &[&str] = &[
    "file",
    "http",
    "https",
    "obsidian",
    "message",
    "x-devonthink-item",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Input exceeds the size cap (S5/GAP-10).
    InputTooLarge {
        field: String,
        bytes: usize,
        max: usize,
    },
    /// Too many requests in the window (S5/GAP-10).
    RateLimited,
    /// URI scheme is not on the allowlist (SC-1: blocks javascript:/data: etc.).
    DisallowedScheme { scheme: String },
    /// URI/name malformed (no scheme, control characters, empty).
    Malformed { reason: String },
}

impl ApiError {
    pub fn message(&self) -> String {
        match self {
            ApiError::InputTooLarge { field, bytes, max } => {
                format!("`{field}` is {bytes} bytes (max {max}).")
            }
            ApiError::RateLimited => "Too many link requests — slow down.".into(),
            ApiError::DisallowedScheme { scheme } => {
                format!("URI scheme `{scheme}:` is not allowed.")
            }
            ApiError::Malformed { reason } => format!("Malformed link request: {reason}."),
        }
    }
}

/// Simple token-bucket rate limiter (S5). Deterministic to test: exhaust the
/// bucket without waiting and the next call is refused.
struct RateLimiter {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl RateLimiter {
    fn new(capacity: u32, refill_per_sec: u32) -> RateLimiter {
        RateLimiter {
            tokens: capacity as f64,
            capacity: capacity as f64,
            refill_per_sec: refill_per_sec as f64,
            last: Instant::now(),
        }
    }
    fn try_acquire(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The bounded local link API (S5). Construct a validated link from an address +
/// name, enforcing size, scheme, and rate limits.
pub struct LinkApi {
    schemes: Vec<String>,
    limiter: RateLimiter,
}

impl LinkApi {
    /// Default: standard schemes, ~20 req/s burst of 20.
    pub fn new() -> LinkApi {
        LinkApi {
            schemes: DEFAULT_SCHEMES.iter().map(|s| s.to_string()).collect(),
            limiter: RateLimiter::new(20, 20),
        }
    }

    /// Trust an additional URI scheme (SC-1 allowlist stays an allowlist). Used to
    /// admit schemes that registered adapters declare as template literals (e.g.
    /// `bear`, `things`) — safe by construction, since the scheme comes from the
    /// adapter config, never from captured input. No-op if already present.
    pub fn allow(&mut self, scheme: &str) {
        let s = scheme.trim().to_ascii_lowercase();
        if !s.is_empty() && !self.schemes.iter().any(|x| x == &s) {
            self.schemes.push(s);
        }
    }

    /// Custom scheme allowlist + rate (tests use a tiny bucket).
    pub fn with_config(schemes: &[&str], capacity: u32, refill_per_sec: u32) -> LinkApi {
        LinkApi {
            schemes: schemes.iter().map(|s| s.to_string()).collect(),
            limiter: RateLimiter::new(capacity, refill_per_sec),
        }
    }

    /// Construct a link (the manifesto's API ask), fully validated and bounded.
    pub fn construct_link(&mut self, uri: &str, name: &str) -> Result<ResourceIdentity, ApiError> {
        // Rate limit FIRST so an oversized-input flood is also throttled (GAP-10).
        if !self.limiter.try_acquire() {
            return Err(ApiError::RateLimited);
        }
        // Size caps (S5).
        if uri.len() > MAX_INPUT_BYTES {
            return Err(ApiError::InputTooLarge {
                field: "uri".into(),
                bytes: uri.len(),
                max: MAX_INPUT_BYTES,
            });
        }
        if name.len() > MAX_INPUT_BYTES {
            return Err(ApiError::InputTooLarge {
                field: "name".into(),
                bytes: name.len(),
                max: MAX_INPUT_BYTES,
            });
        }
        // Encoding normalization + control-char rejection (SC-1).
        let uri = uri.trim();
        if uri.is_empty() {
            return Err(ApiError::Malformed {
                reason: "empty uri".into(),
            });
        }
        if uri.chars().any(|c| c.is_control()) || name.chars().any(|c| c.is_control()) {
            return Err(ApiError::Malformed {
                reason: "control characters not allowed".into(),
            });
        }
        // Scheme allowlist (SC-1): blocks javascript:, data:, etc.
        let scheme = uri
            .split_once(':')
            .map(|(s, _)| s.to_ascii_lowercase())
            .ok_or_else(|| ApiError::Malformed {
                reason: "uri has no scheme".into(),
            })?;
        if !self.schemes.iter().any(|s| s == &scheme) {
            return Err(ApiError::DisallowedScheme { scheme });
        }
        Ok(ResourceIdentity::new(name.trim(), uri))
    }
}

impl Default for LinkApi {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_a_valid_link() {
        let mut api = LinkApi::new();
        let id = api
            .construct_link("obsidian://vault/Note.md", "Note.md")
            .unwrap();
        assert_eq!(id.uri, "obsidian://vault/Note.md");
        assert_eq!(id.name, "Note.md");
    }

    #[test]
    fn rejects_oversized_input() {
        let mut api = LinkApi::new();
        let big = format!("file://{}", "a".repeat(MAX_INPUT_BYTES));
        match api.construct_link(&big, "x").unwrap_err() {
            ApiError::InputTooLarge { field, .. } => assert_eq!(field, "uri"),
            other => panic!("expected InputTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn rejects_disallowed_scheme() {
        // SC-1: a javascript: URI must be refused (injection vector).
        let mut api = LinkApi::new();
        assert_eq!(
            api.construct_link("javascript:alert(1)", "x").unwrap_err(),
            ApiError::DisallowedScheme {
                scheme: "javascript".into()
            }
        );
    }

    #[test]
    fn allow_admits_an_adapter_declared_scheme() {
        // The config-driven fix: a captured bear:// link is rejected until the
        // scheme (declared by the Bear adapter) is trusted, then it constructs.
        let mut api = LinkApi::new();
        assert_eq!(
            api.construct_link("bear://x-callback-url/open-note?title=x", "x")
                .unwrap_err(),
            ApiError::DisallowedScheme {
                scheme: "bear".into()
            }
        );
        api.allow("bear");
        assert!(api
            .construct_link("bear://x-callback-url/open-note?title=x", "x")
            .is_ok());
        // …but trusting bear: does NOT open the door to javascript: (S5 intact).
        assert!(matches!(
            api.construct_link("javascript:alert(1)", "x").unwrap_err(),
            ApiError::DisallowedScheme { .. }
        ));
    }

    #[test]
    fn rejects_control_characters() {
        let mut api = LinkApi::new();
        assert!(matches!(
            api.construct_link("file:///a\nb", "x").unwrap_err(),
            ApiError::Malformed { .. }
        ));
    }

    #[test]
    fn rate_limits_a_flood() {
        // S5/GAP-10: a tiny bucket (3, no meaningful refill in test time) -> the
        // 4th rapid request is refused.
        let mut api = LinkApi::with_config(&["file"], 3, 0);
        for _ in 0..3 {
            assert!(api.construct_link("file:///a", "a").is_ok());
        }
        assert_eq!(
            api.construct_link("file:///a", "a").unwrap_err(),
            ApiError::RateLimited
        );
    }
}
