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

/// Default scheme allowlist (SC-1). Extend via `LinkPolicy::allow`. `file` stays
/// on it because the built-in Finder adapter produces `file://` links; `validate`
/// admits only LOCAL ones (no remote host, no UNC path).
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
    /// A `file:` URI names another machine (`file://server/…`, or a UNC spelling
    /// such as `file:////server/…`). Opening it contacts that host — SMB on
    /// Windows (NTLM credential leak), a share mount on macOS.
    RemoteFile { host: String },
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
            ApiError::RemoteFile { host } => {
                let target = if host.is_empty() {
                    "a network location".to_string()
                } else {
                    format!("the network host `{host}`")
                };
                format!(
                    "`file:` links must point at this computer, but this one names \
                     {target}; opening it would connect to that machine."
                )
            }
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

/// Schemes that are NEVER accepted, whatever the allowlist says (SC-1). The
/// allowlist can be widened by adapter config (a user-editable `adapters.json`),
/// so config is not a trust boundary on its own; this list is the floor it can't
/// lower. Checked BEFORE the allowlist.
pub const DENIED_SCHEMES: &[&str] = &[
    // script execution in whatever handles the link
    "javascript",
    "vbscript",
    "livescript",
    // inline / opaque content that renders attacker-supplied payloads
    "data",
    "blob",
    "view-source",
    "jar",
    // Windows protocol handlers with a history of drive-by code execution
    // (Follina ms-msdt, search-ms / search file-lure, ms-officecmd, ms-appinstaller)
    "ms-msdt",
    "search-ms",
    "search",
    "ms-officecmd",
    "ms-appinstaller",
];

/// Bidirectional formatting characters that can make a link RENDER differently
/// from what it opens (Trojan-Source-style spoofing). `char::is_control` only
/// covers Cc, so these (Cf) need their own check.
fn is_bidi_override(c: char) -> bool {
    // embeddings/overrides (LRE RLE PDF LRO RLO) + isolates (LRI RLI FSI PDI)
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Characters refused anywhere in a URI: control chars (C0/C1), every bidi
/// formatting char (overrides + LRM/RLM/ALM marks), and the Unicode line/paragraph
/// separators. A well-formed URI percent-encodes anything like this.
fn forbidden_in_uri(c: char) -> bool {
    c.is_control()
        || is_bidi_override(c)
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}'
        )
}

/// Characters refused in a display name: control chars + bidi overrides/isolates.
/// Plain LRM/RLM marks are allowed — they occur legitimately in right-to-left page
/// titles and cannot reorder text on their own.
fn forbidden_in_name(c: char) -> bool {
    c.is_control() || is_bidi_override(c)
}

/// RFC 3986 §3.1: `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
fn is_valid_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn is_path_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// The first segment of a UNC-style path (`\\server\share` / `//server/share`).
fn unc_host(s: &str) -> String {
    s.trim_start_matches(is_path_separator)
        .split(is_path_separator)
        .next()
        .unwrap_or("")
        .to_string()
}

/// The remote host a `file:` URI names, or `None` if it names this computer.
/// `after_scheme` is everything after `file:`. Local means: no authority, an empty
/// one (`file:///p`), or `localhost` in any case (`file://localhost/p`), AND a path
/// that does not itself start with two separators — `file:////server/share`,
/// `file:\\server\share` and `file:///%2F%2Fserver` are UNC paths that Windows
/// resolves over the network. The authority is compared as written (an encoded or
/// port-qualified `localhost` is refused); the path is percent-decoded first.
fn remote_file_host(after_scheme: &str) -> Option<String> {
    let rest = after_scheme.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.strip_prefix("//") {
        Some(a) => {
            let end = a.find('/').unwrap_or(a.len());
            (&a[..end], &a[end..])
        }
        None => ("", rest),
    };
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return Some(if authority.starts_with(is_path_separator) {
            unc_host(authority)
        } else {
            authority.to_string()
        });
    }
    let path = crate::adapter::percent_decode(path);
    let mut lead = path.chars();
    match (lead.next(), lead.next()) {
        (Some(a), Some(b)) if is_path_separator(a) && is_path_separator(b) => Some(unc_host(&path)),
        _ => None,
    }
}

fn check_size(field: &str, value: &str) -> Result<(), ApiError> {
    if value.len() > MAX_INPUT_BYTES {
        return Err(ApiError::InputTooLarge {
            field: field.into(),
            bytes: value.len(),
            max: MAX_INPUT_BYTES,
        });
    }
    Ok(())
}

/// The link validation policy (S5/SC-1), shared by EVERY boundary a link crosses:
/// capture (`LinkApi::construct_link`), import (`UbilinkService::import`) and open
/// (`adapter::open_uri`). It needs no capture context — just the scheme allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPolicy {
    schemes: Vec<String>,
}

impl LinkPolicy {
    /// The default allowlist (`DEFAULT_SCHEMES`).
    pub fn new() -> LinkPolicy {
        Self::with_schemes(DEFAULT_SCHEMES)
    }

    /// A custom allowlist (tests).
    pub fn with_schemes(schemes: &[&str]) -> LinkPolicy {
        let mut p = LinkPolicy { schemes: vec![] };
        for s in schemes {
            p.allow(s);
        }
        p
    }

    /// Add a scheme to the allowlist. Used for schemes that registered adapters
    /// declare (e.g. `bear`, `things`). Adapter config is user-editable, so this is
    /// NOT "safe by construction": a scheme on `DENIED_SCHEMES` is still refused by
    /// `validate`, and every other check still applies. No-op if already present.
    pub fn allow(&mut self, scheme: &str) {
        let s = scheme.trim().to_ascii_lowercase();
        if !s.is_empty() && !self.schemes.iter().any(|x| x == &s) {
            self.schemes.push(s);
        }
    }

    /// Validate a link EXACTLY as given (no trimming/normalization — callers that
    /// normalize do so first, so what is validated is what is stored or launched).
    /// `name` may be empty (open has no name to check). Checks, in order: size caps,
    /// non-empty uri, forbidden characters, leading `-`, scheme syntax, the hard
    /// deny-list, the allowlist, then — for `file:` — that the link names this
    /// computer (empty host or `localhost`; no remote host, no UNC path). Because
    /// this policy is shared, the local-file rule holds for capture, import and open.
    pub fn validate(&self, uri: &str, name: &str) -> Result<(), ApiError> {
        check_size("uri", uri)?;
        check_size("name", name)?;
        if uri.is_empty() {
            return Err(ApiError::Malformed {
                reason: "empty uri".into(),
            });
        }
        if uri.chars().any(forbidden_in_uri) || name.chars().any(forbidden_in_name) {
            return Err(ApiError::Malformed {
                reason: "control or bidi-override characters not allowed".into(),
            });
        }
        // A leading '-' would be parsed as an option by `open`/`xdg-open`.
        if uri.starts_with('-') {
            return Err(ApiError::Malformed {
                reason: "uri must not start with '-'".into(),
            });
        }
        let (scheme, after_scheme) = uri.split_once(':').ok_or_else(|| ApiError::Malformed {
            reason: "uri has no scheme".into(),
        })?;
        if !is_valid_scheme(scheme) {
            return Err(ApiError::Malformed {
                reason: "uri scheme is not valid".into(),
            });
        }
        let scheme = scheme.to_ascii_lowercase();
        if DENIED_SCHEMES.contains(&scheme.as_str()) || !self.schemes.iter().any(|s| s == &scheme) {
            return Err(ApiError::DisallowedScheme { scheme });
        }
        if scheme == "file" {
            if let Some(host) = remote_file_host(after_scheme) {
                return Err(ApiError::RemoteFile { host });
            }
        }
        Ok(())
    }
}

impl Default for LinkPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// The bounded local link API (S5). Construct a validated link from an address +
/// name, enforcing size, scheme, and rate limits.
pub struct LinkApi {
    policy: LinkPolicy,
    limiter: RateLimiter,
}

impl LinkApi {
    /// Default: standard schemes, ~20 req/s burst of 20.
    pub fn new() -> LinkApi {
        LinkApi {
            policy: LinkPolicy::new(),
            limiter: RateLimiter::new(20, 20),
        }
    }

    /// Trust an additional URI scheme (see `LinkPolicy::allow` — the hard
    /// deny-list and every other check still apply).
    pub fn allow(&mut self, scheme: &str) {
        self.policy.allow(scheme);
    }

    /// Custom scheme allowlist + rate (tests use a tiny bucket).
    pub fn with_config(schemes: &[&str], capacity: u32, refill_per_sec: u32) -> LinkApi {
        LinkApi {
            policy: LinkPolicy::with_schemes(schemes),
            limiter: RateLimiter::new(capacity, refill_per_sec),
        }
    }

    /// The validation policy this API enforces — shared with import and open.
    pub fn policy(&self) -> &LinkPolicy {
        &self.policy
    }

    /// Construct a link (the manifesto's API ask), fully validated and bounded.
    pub fn construct_link(&mut self, uri: &str, name: &str) -> Result<ResourceIdentity, ApiError> {
        // Rate limit FIRST so an oversized-input flood is also throttled (GAP-10).
        if !self.limiter.try_acquire() {
            return Err(ApiError::RateLimited);
        }
        // Size cap on the raw input (S5), then normalize: capture output is trimmed
        // (osascript appends a newline) before the shared validation runs.
        check_size("uri", uri)?;
        let uri = uri.trim();
        self.policy.validate(uri, name)?;
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
    fn deny_listed_schemes_stay_blocked_even_if_config_allows_them() {
        // Adapter config is user-editable (adapters.json), so `allow` must never be
        // able to re-open the classic injection schemes.
        let mut api = LinkApi::new();
        for s in [
            "javascript",
            "data",
            "vbscript",
            "JavaScript",
            "blob",
            "ms-msdt",
        ] {
            api.allow(s);
        }
        for uri in [
            "javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "blob:https://x/1",
            "ms-msdt:/id PCWDiagnostic",
        ] {
            assert!(
                matches!(
                    api.construct_link(uri, "x"),
                    Err(ApiError::DisallowedScheme { .. })
                ),
                "{uri} must be refused even when config allows its scheme"
            );
        }
    }

    #[test]
    fn rejects_a_leading_dash_so_it_cannot_become_a_cli_option() {
        // `open -a Calculator`-style smuggling: anything starting with '-' is refused
        // as malformed, whatever the allowlist says.
        let mut api = LinkApi::new();
        api.allow("-a");
        for uri in ["-a:Calculator", "-foo", " -a:Calculator"] {
            assert!(
                matches!(
                    api.construct_link(uri, "x"),
                    Err(ApiError::Malformed { .. })
                ),
                "{uri:?} must be refused as malformed"
            );
        }
    }

    #[test]
    fn rejects_an_invalid_scheme_syntax() {
        // RFC 3986: scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ).
        let mut api = LinkApi::new();
        api.allow("1http");
        assert!(matches!(
            api.construct_link("1http://x", "x"),
            Err(ApiError::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_bidi_override_characters() {
        // Trojan-Source-style spoofing: a RIGHT-TO-LEFT OVERRIDE makes
        // "https://x/\u{202E}fdp.exe" render as "https://x/exe.pdf".
        let mut api = LinkApi::new();
        for uri in [
            "https://x/\u{202E}fdp.exe",
            "https://x/\u{2066}a",
            "https://x/\u{200F}a",
            "https://x/\u{2028}a",
        ] {
            assert!(
                matches!(
                    api.construct_link(uri, "x"),
                    Err(ApiError::Malformed { .. })
                ),
                "{uri:?} must be refused"
            );
        }
        // …and an override in the display name is refused too.
        assert!(matches!(
            api.construct_link("https://x/a", "report\u{202E}fdp.exe"),
            Err(ApiError::Malformed { .. })
        ));
        // A plain right-to-left MARK in a title (common in Hebrew/Arabic page
        // titles) is not a spoofing primitive and stays allowed in the name.
        assert!(api
            .construct_link("https://x/a", "\u{05E9}\u{05DC}\u{05D5}\u{05DD}\u{200F}")
            .is_ok());
    }

    #[test]
    fn policy_validates_without_any_capture_context() {
        // The shared validator used by capture, import and open: no rate limiter,
        // no adapter — just the allowlist + the hard checks.
        let mut policy = LinkPolicy::new();
        assert_eq!(
            policy.validate("https://example.com/a?b=1&c=2", "x"),
            Ok(())
        );
        assert_eq!(policy.validate("file:///Users/me/My Note.md", ""), Ok(()));
        assert_eq!(
            policy.validate("bear://x-callback-url/open-note?id=1", "x"),
            Err(ApiError::DisallowedScheme {
                scheme: "bear".into()
            })
        );
        policy.allow("bear");
        assert_eq!(
            policy.validate("bear://x-callback-url/open-note?id=1", "x"),
            Ok(())
        );
        policy.allow("javascript");
        assert!(matches!(
            policy.validate("javascript:alert(1)", "x"),
            Err(ApiError::DisallowedScheme { .. })
        ));
        // validation does not normalize: surrounding whitespace / a trailing newline
        // are refused (import + open store/launch exactly what they validate).
        assert!(policy.validate(" https://x", "x").is_err());
        assert!(policy.validate("https://x\n", "x").is_err());
        assert!(matches!(
            policy.validate("", "x"),
            Err(ApiError::Malformed { .. })
        ));
        let big = format!("https://{}", "a".repeat(MAX_INPUT_BYTES));
        assert!(matches!(
            policy.validate(&big, "x"),
            Err(ApiError::InputTooLarge { .. })
        ));
        assert!(matches!(
            policy.validate("https://x", &"n".repeat(MAX_INPUT_BYTES + 1)),
            Err(ApiError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn file_links_must_point_at_this_computer() {
        // A `file:` link with a host is a network share: on Windows opening
        // `file://server/share/x` (or `file:////server/share`) goes over SMB and can
        // leak NTLM credentials; on macOS it can mount a share. Only an empty host
        // or `localhost` names this computer.
        let policy = LinkPolicy::new();
        for local in [
            "file:///Users/me/a.md",
            "file://localhost/Users/me/a.md",
            "file://LocalHost/Users/me/a.md",
            "file:///C:/x.txt",
            "file:/Users/me/a.md",
            "file:///Users/me/My%20Note.md?x=1#y",
            "FILE:///Users/me/a.md",
        ] {
            assert_eq!(policy.validate(local, ""), Ok(()), "{local} is local");
        }
        for remote in [
            "file://server/share/x",
            "file://SERVER",
            "file://evil.example/share/x?q",
            "file://user@server/x",
            "file://localhost:445/x",
            "file://127.0.0.1/x",
            // UNC spellings: extra slashes, backslashes, and encoded separators
            "file:////server/share/x",
            "file://///server/share/x",
            "file://\\\\server\\share\\x",
            "file:\\\\server\\share\\x",
            "file:/\\server\\share",
            "file://localhost//server/share",
            "file:///%2F%2Fserver/share",
            "file:///%5C%5Cserver/share",
            "file:%2F%2Fserver/share",
        ] {
            let err = policy
                .validate(remote, "")
                .expect_err(&format!("{remote} names a remote host"));
            let msg = err.message();
            assert!(
                msg.contains("this computer"),
                "{remote}: unclear message {msg:?}"
            );
        }
        // …and allowing `file` again (e.g. from adapter config) doesn't lift it.
        let mut p = LinkPolicy::new();
        p.allow("file");
        assert!(p.validate("file://server/share/x", "").is_err());
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
