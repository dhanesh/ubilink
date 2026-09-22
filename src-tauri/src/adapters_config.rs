// Satisfies: B3 (add app support WITHOUT forking core — an adapter is data, not
// Rust), T5 (the addressing layer), B1 (live capture), U3 (explained degradation).
// See .manifold/linking-manifesto-app.md
//
// Declarative, config-driven adapters. An adapter is a spec: which frontmost app it
// matches, the `osascript` to run, how to split that output into named fields, and
// templates that build the link URI + name from those fields. So adding an app — any
// of the apps that support ubiquitous linking — is a JSON
// entry, not a code change and not a recompile (B3). Built-ins ship embedded; users
// drop an `adapters.json` to extend or override.
//
// Trust (DECISIONS D14): adapter JSON is TRUSTED LOCAL CONFIGURATION, not a plugin.
// Its `script` runs via `osascript` with the user's privileges and Ubilink's TCC
// grants, outside the Node plugin sandbox. Only the embedded defaults may be
// `builtin`; user entries are always `user_defined` and disableable.
//
// Functional core: `render` (the template engine) and `ConfiguredAdapter::parse` are
// pure and fully unit-tested; only the single `osascript` call needs a real Mac.

use crate::adapter::{open_plan_for, CaptureFailure, OpenPlan, ResourceAdapter};
use crate::capabilities::Platform;
use crate::live_capture::CommandRunner;
use crate::resource::ResourceIdentity;
use serde::Deserialize;
use std::collections::HashMap;

/// One declarative adapter. Deserialized from the embedded defaults or a user file.
#[derive(Debug, Clone, Deserialize)]
pub struct AdapterSpec {
    /// Frontmost-app name to match (case-insensitive), e.g. "Safari", "Bear".
    pub app: String,
    /// The AppleScript run via `osascript -e` to read the active resource.
    pub script: String,
    /// Delimiter that splits the script's output into `fields`. Omit for a single field.
    #[serde(default)]
    pub delimiter: Option<String>,
    /// Names bound to the split output parts, referenced by the templates below.
    pub fields: Vec<String>,
    /// Template for the link URI, e.g. "bear://x-callback-url/open-note?title={title|urlencode}".
    pub uri: String,
    /// Template for the human name.
    pub name: String,
    /// Optional template for a path identifier (RT-3), e.g. Finder's "{path}".
    #[serde(default)]
    pub path: Option<String>,
    /// URI schemes this adapter may emit when the `uri` is DYNAMIC (e.g. built from
    /// a clipboard value, so no literal scheme can be inferred). Declared schemes are
    /// trusted by the bounded API (S5). Literal-template adapters don't need this.
    #[serde(default)]
    pub schemes: Vec<String>,
    /// Whether capturing needs OS automation permission (live apps do).
    #[serde(default = "default_true")]
    pub needs_permission: bool,
    /// Built-in = a default-macOS app (Safari, Finder) shipped as core, always on
    /// and not removable. Everything else is listed in the plugin manager and
    /// toggleable on/off. Only honoured for the EMBEDDED defaults: a user
    /// `adapters.json` entry is always forced to `false` (D14), so it can't make
    /// itself impossible to disable.
    #[serde(default)]
    pub builtin: bool,
    /// True for entries that came from the user's `adapters.json` (including a
    /// user override of a built-in). Never read from JSON; set by
    /// `extend_from_json`. These are trusted local config running with the
    /// user's own privileges, NOT sandboxed plugins (D14).
    #[serde(skip)]
    pub user_defined: bool,
}

fn default_true() -> bool {
    true
}

/// Render a template against field values. Supports `{field}` and `{field|transform}`
/// where transform is `urlencode` or `basename`. Pure — no I/O.
fn render(template: &str, fields: &HashMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').ok_or("unclosed '{' in template")? + open;
        let expr = &rest[open + 1..close];
        let (field, transform) = match expr.split_once('|') {
            Some((f, t)) => (f.trim(), Some(t.trim())),
            None => (expr.trim(), None),
        };
        let val = fields
            .get(field)
            .ok_or_else(|| format!("template references unknown field {{{field}}}"))?;
        let rendered = match transform {
            None => val.clone(),
            Some("urlencode") => urlencode(val),
            Some("basename") => basename(val),
            // For values that may be a markdown link `[text](url)` (e.g. some apps'
            // "Copy Link" puts markdown on the clipboard); pass through if not.
            Some("mdurl") => md_url(val),
            Some("mdtext") => md_text(val),
            Some(other) => return Err(format!("unknown template transform '{other}'")),
        };
        out.push_str(&rendered);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Percent-encode everything outside the unreserved set (RFC 3986).
fn urlencode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// The final path component (Finder paths -> a display name).
fn basename(s: &str) -> String {
    s.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(s)
        .to_string()
}

/// The URL out of a markdown link `[text](url)`, or the input unchanged if it isn't one.
fn md_url(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('[') {
        if let (Some(mid), true) = (s.find("]("), s.ends_with(')')) {
            return s[mid + 2..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// The text out of a markdown link `[text](url)`, or the input unchanged if it isn't one.
fn md_text(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('[') {
        if let Some(mid) = s.find("](") {
            return s[1..mid].to_string();
        }
    }
    s.to_string()
}

/// A live adapter built from a spec + a command runner (the I/O seam).
pub struct ConfiguredAdapter<R: CommandRunner> {
    spec: AdapterSpec,
    runner: R,
}

impl<R: CommandRunner> ConfiguredAdapter<R> {
    pub fn new(spec: AdapterSpec, runner: R) -> ConfiguredAdapter<R> {
        ConfiguredAdapter { spec, runner }
    }

    /// Pure: turn raw script output into a ResourceIdentity via the spec's templates.
    fn parse(&self, raw: &str) -> Result<ResourceIdentity, CaptureFailure> {
        let fail = |msg: String| CaptureFailure::AdapterError { explanation: msg };
        let line = raw.lines().next().unwrap_or("");
        if line.trim().is_empty() {
            return Err(fail(format!(
                "{} returned no active resource — open something there and try again.",
                self.spec.app
            )));
        }
        // Bind output -> named fields.
        let mut fields: HashMap<String, String> = HashMap::new();
        match &self.spec.delimiter {
            Some(delim) => {
                let parts: Vec<&str> = line
                    .splitn(self.spec.fields.len(), delim.as_str())
                    .collect();
                if parts.len() < self.spec.fields.len() {
                    return Err(fail(format!(
                        "{} returned an unexpected response.",
                        self.spec.app
                    )));
                }
                for (name, val) in self.spec.fields.iter().zip(parts) {
                    fields.insert(name.clone(), val.trim().to_string());
                }
            }
            None => {
                let name = self
                    .spec
                    .fields
                    .first()
                    .ok_or_else(|| fail("adapter has no fields".into()))?;
                fields.insert(name.clone(), line.trim().to_string());
            }
        }
        let render_field =
            |tpl: &str| render(tpl, &fields).map_err(|e| fail(format!("{}: {e}", self.spec.app)));
        let uri = render_field(&self.spec.uri)?;
        if uri.trim().is_empty() {
            return Err(fail(format!("{} produced an empty link.", self.spec.app)));
        }
        let name = render_field(&self.spec.name)?;
        let mut id = ResourceIdentity::new(name, uri);
        if let Some(path_tpl) = &self.spec.path {
            let p = render_field(path_tpl)?;
            if !p.is_empty() {
                id = id.with_path(p);
            }
        }
        Ok(id)
    }
}

/// Strip osascript's framing from a script error so the user reads a sentence,
/// not AppleScript internals: `77:82: execution error: <message> (-1700)` becomes
/// `<message>`. Anything not in exactly that shape is returned untouched, so a
/// plain runner error (a timeout, a missing binary) is never mangled.
fn clean_script_error(raw: &str) -> String {
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let Some((offsets, rest)) = raw.trim().split_once(": execution error: ") else {
        return raw.to_string();
    };
    let framed = offsets
        .split_once(':')
        .is_some_and(|(from, to)| all_digits(from) && all_digits(to));
    if !framed {
        return raw.to_string();
    }
    // Drop a trailing Apple event code like " (-1700)", and only that.
    let message = match rest.rsplit_once(" (") {
        Some((msg, code))
            if code
                .strip_suffix(')')
                .and_then(|c| c.strip_prefix('-'))
                .is_some_and(all_digits) =>
        {
            msg
        }
        _ => rest,
    };
    message.trim().to_string()
}

/// The user-facing explanation for a failed capture script. One failure gets its
/// own message because it has a known fix: an adapter that UI-scripts an app
/// through System Events (Bear) needs Accessibility, and macOS reports its absence
/// as "osascript is not allowed assistive access" — accurate, but it never names
/// the permission. Everything else gets the cleaned script error.
fn explain_script_error(app: &str, raw: &str) -> String {
    if raw.contains("not allowed assistive access") {
        return format!(
            "Capturing from {app} needs Accessibility permission, because Ubilink reads \
             {app}'s menus. Grant it under Permissions."
        );
    }
    format!(
        "Couldn't read the active resource from {app}: {}",
        clean_script_error(raw)
    )
}

impl<R: CommandRunner> ResourceAdapter for ConfiguredAdapter<R> {
    fn id(&self) -> &str {
        &self.spec.app
    }
    fn needs_permission(&self) -> bool {
        self.spec.needs_permission
    }
    fn capture(&self) -> Result<ResourceIdentity, CaptureFailure> {
        match self.runner.run("osascript", &["-e", &self.spec.script]) {
            Ok(out) => self.parse(&out),
            Err(e) => Err(CaptureFailure::AdapterError {
                explanation: explain_script_error(&self.spec.app, &e),
            }),
        }
    }
    fn open_plan(&self, resource: &ResourceIdentity) -> OpenPlan {
        // The shared no-shell, no-option opener (`open -- <uri>` on macOS).
        open_plan_for(Platform::current(), &resource.uri)
    }
}

/// The set of known adapters: built-in defaults plus any user additions.
pub struct AdapterRegistry {
    specs: Vec<AdapterSpec>,
}

impl AdapterRegistry {
    /// Parse the EMBEDDED defaults. Private on purpose: this path honours
    /// `builtin`, so it must never be fed user JSON (use `extend_from_json`).
    fn from_json(json: &str) -> Result<AdapterRegistry, String> {
        let specs: Vec<AdapterSpec> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        Ok(AdapterRegistry { specs })
    }

    /// The built-in adapters shipped with ubilink (embedded at compile time).
    pub fn defaults() -> AdapterRegistry {
        Self::from_json(include_str!("adapters.default.json"))
            .expect("built-in adapters.default.json must be valid")
    }

    /// Merge the user's local `adapters.json`; a user entry replaces a built-in with
    /// the same app (B3: extend or fix coverage without touching core).
    ///
    /// Trust (D14): these entries are trusted LOCAL configuration — their `script`
    /// runs via `osascript` with the user's own privileges and TCC grants. They are
    /// not sandboxed plugins. So a user entry is never `builtin` (whatever the JSON
    /// says, including when it overrides a built-in): it stays disableable.
    pub fn extend_from_json(&mut self, json: &str) -> Result<usize, String> {
        let user: Vec<AdapterSpec> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let n = user.len();
        for mut spec in user {
            spec.builtin = false;
            spec.user_defined = true;
            self.specs
                .retain(|s| !s.app.eq_ignore_ascii_case(&spec.app));
            self.specs.insert(0, spec);
        }
        Ok(n)
    }

    /// Find the adapter spec for a frontmost-app name (case-insensitive).
    pub fn for_app(&self, frontmost: &str) -> Option<&AdapterSpec> {
        let f = frontmost.trim();
        self.specs.iter().find(|s| s.app.eq_ignore_ascii_case(f))
    }

    /// Every supported app name (for diagnostics / docs).
    pub fn apps(&self) -> Vec<&str> {
        self.specs.iter().map(|s| s.app.as_str()).collect()
    }

    /// All adapter specs (the plugin manager lists these: app + builtin flag).
    pub fn specs(&self) -> &[AdapterSpec] {
        &self.specs
    }

    /// The literal URI schemes the adapters declare (e.g. `bear`, `things`, `file`).
    /// Only templates that BEGIN with a literal scheme count — a `{url}` template
    /// has a dynamic scheme (http/https), which the bounded API already trusts. The
    /// service feeds these to `LinkApi::allow` so config-added apps pass S5.
    pub fn declared_schemes(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |s: String| {
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        };
        for spec in &self.specs {
            // a literal-scheme template (bear://…) contributes its scheme automatically…
            if let Some(scheme) = literal_scheme(&spec.uri) {
                add(scheme);
            }
            // …and a dynamic-uri adapter declares its scheme(s) explicitly.
            for s in &spec.schemes {
                add(s.to_ascii_lowercase());
            }
        }
        out
    }
}

/// Extract a literal scheme from a URI template that starts with one (not a `{field}`).
fn literal_scheme(uri_template: &str) -> Option<String> {
    let before = uri_template.split(':').next()?;
    if before.is_empty() || before.contains('{') || before.contains('/') {
        return None; // dynamic scheme (e.g. "{url}") or not a scheme prefix
    }
    if before
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        Some(before.to_ascii_lowercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRunner(Result<String, String>);
    impl CommandRunner for FakeRunner {
        fn run(&self, _p: &str, _a: &[&str]) -> Result<String, String> {
            self.0.clone()
        }
    }
    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn render_plain_and_transforms() {
        let f = fields(&[
            ("url", "https://x/y"),
            ("title", "A B & C"),
            ("path", "/Users/me/My Doc.pdf"),
        ]);
        assert_eq!(render("{url}", &f).unwrap(), "https://x/y");
        assert_eq!(
            render("t={title|urlencode}", &f).unwrap(),
            "t=A%20B%20%26%20C"
        );
        assert_eq!(render("{path|basename}", &f).unwrap(), "My Doc.pdf");
        assert_eq!(
            render("file://{path}", &f).unwrap(),
            "file:///Users/me/My Doc.pdf"
        );
    }

    #[test]
    fn markdown_transforms_handle_both_link_and_plain() {
        // A markdown link splits into url/text…
        let md = fields(&[(
            "link",
            "[Daily Memories](bear://x-callback-url/open-note?id=ABC)",
        )]);
        assert_eq!(
            render("{link|mdurl}", &md).unwrap(),
            "bear://x-callback-url/open-note?id=ABC"
        );
        assert_eq!(render("{link|mdtext}", &md).unwrap(), "Daily Memories");
        // …a plain URL passes through unchanged (graceful fallback).
        let plain = fields(&[("link", "bear://x-callback-url/open-note?id=ABC")]);
        assert_eq!(
            render("{link|mdurl}", &plain).unwrap(),
            "bear://x-callback-url/open-note?id=ABC"
        );
        assert_eq!(
            render("{link|mdtext}", &plain).unwrap(),
            "bear://x-callback-url/open-note?id=ABC"
        );
    }

    #[test]
    fn md_url_and_md_text_cover_edges_directly() {
        // Well-formed markdown link: url vs. text.
        assert_eq!(md_url("[Title](bear://x?id=1)"), "bear://x?id=1");
        assert_eq!(md_text("[Title](bear://x?id=1)"), "Title");

        // Not a markdown link → passthrough, both transforms.
        assert_eq!(md_url("bear://x?id=1"), "bear://x?id=1");
        assert_eq!(md_text("bear://x?id=1"), "bear://x?id=1");

        // Surrounding whitespace is trimmed before parsing.
        assert_eq!(md_url("  [t](u)  "), "u");
        assert_eq!(md_text("  [t](u)  "), "t");

        // A url that itself contains ')' — the LAST char closes the link.
        assert_eq!(md_url("[t](http://x/(a))"), "http://x/(a)");
        assert_eq!(md_text("[t](http://x/(a))"), "t");

        // Empty link "[]()" yields empty parts, not the literal.
        assert_eq!(md_url("[]()"), "");
        assert_eq!(md_text("[]()"), "");

        // Asymmetry worth pinning: md_url REQUIRES a trailing ')', so a truncated
        // link falls back to the input untouched; md_text only needs "](" and so
        // still extracts the label. (Documents behaviour, not an aspiration.)
        assert_eq!(md_url("[t](u"), "[t](u");
        assert_eq!(md_text("[t](u"), "t");

        // "](" present but no leading '[' → neither treats it as a link.
        assert_eq!(md_url("a](b"), "a](b");
        assert_eq!(md_text("a](b"), "a](b");
    }

    #[test]
    fn render_errors_are_explained() {
        let f = fields(&[("a", "1")]);
        assert!(render("{missing}", &f)
            .unwrap_err()
            .contains("unknown field"));
        assert!(render("{a|nope}", &f)
            .unwrap_err()
            .contains("unknown template transform"));
        assert!(render("{a", &f).unwrap_err().contains("unclosed"));
    }

    #[test]
    fn browser_spec_parses_url_and_title() {
        let spec = AdapterSpec {
            app: "Safari".into(),
            script: "…".into(),
            delimiter: Some("\t".into()),
            fields: vec!["url".into(), "title".into()],
            uri: "{url}".into(),
            name: "{title}".into(),
            path: None,
            schemes: vec![],
            needs_permission: true,
            builtin: false,
            user_defined: false,
        };
        let a = ConfiguredAdapter::new(
            spec,
            FakeRunner(Ok("https://example.com/x\tExample\n".into())),
        );
        let id = a.capture().unwrap();
        assert_eq!(id.uri, "https://example.com/x");
        assert_eq!(id.name, "Example");
    }

    #[test]
    fn finder_spec_builds_file_uri_with_path_and_basename_name() {
        let spec = AdapterSpec {
            app: "Finder".into(),
            script: "…".into(),
            delimiter: None,
            fields: vec!["path".into()],
            uri: "file://{path}".into(),
            name: "{path|basename}".into(),
            path: Some("{path}".into()),
            schemes: vec![],
            needs_permission: true,
            builtin: false,
            user_defined: false,
        };
        let a = ConfiguredAdapter::new(spec, FakeRunner(Ok("/Users/me/Docs/Report.pdf".into())));
        let id = a.capture().unwrap();
        assert_eq!(id.uri, "file:///Users/me/Docs/Report.pdf");
        assert_eq!(id.name, "Report.pdf");
        assert_eq!(id.path.as_deref(), Some("/Users/me/Docs/Report.pdf"));
    }

    #[test]
    fn builtin_finder_links_pass_the_local_file_policy() {
        // Regression guard for the local-`file:` rule: the shipped Finder adapter
        // renders `file://{path}` from a POSIX path (leading `/`), i.e. an empty
        // host, so its links must keep validating.
        let spec = AdapterRegistry::defaults()
            .for_app("Finder")
            .expect("Finder ships")
            .clone();
        let policy = crate::api::LinkPolicy::new();
        for path in ["/Users/me/Docs/Report.pdf", "/Volumes/USB/My Doc.pdf", "/"] {
            let id = ConfiguredAdapter::new(spec.clone(), FakeRunner(Ok(path.into())))
                .capture()
                .unwrap();
            assert_eq!(policy.validate(&id.uri, &id.name), Ok(()), "{}", id.uri);
        }
    }

    #[test]
    fn open_plan_uses_the_safe_platform_opener() {
        // A configured adapter must open links the same no-shell, no-option way as
        // every other path (`adapter::open_plan_for`): on macOS `open -- <uri>`, so
        // a URI can never be parsed as an `open` option.
        use crate::capabilities::Platform;
        let spec = AdapterRegistry::defaults()
            .for_app("Safari")
            .unwrap()
            .clone();
        let a = ConfiguredAdapter::new(spec, FakeRunner(Ok(String::new())));
        let uri = "https://example.com/a?b=1";
        let plan = a.open_plan(&ResourceIdentity::new("n", uri));
        assert_eq!(
            plan,
            crate::adapter::open_plan_for(Platform::current(), uri)
        );
        if Platform::current() == Platform::MacOS {
            assert_eq!(plan.program, "open");
            assert_eq!(plan.args, vec!["--".to_string(), uri.to_string()]);
        }
    }

    #[test]
    fn urlencode_transform_escapes_a_title_into_a_scheme_uri() {
        let spec = AdapterSpec {
            app: "Bear".into(),
            script: "…".into(),
            delimiter: None,
            fields: vec!["title".into()],
            uri: "bear://x-callback-url/open-note?title={title|urlencode}".into(),
            name: "{title}".into(),
            path: None,
            schemes: vec![],
            needs_permission: true,
            builtin: false,
            user_defined: false,
        };
        let a = ConfiguredAdapter::new(spec, FakeRunner(Ok("My Note: ideas".into())));
        let id = a.capture().unwrap();
        assert_eq!(
            id.uri,
            "bear://x-callback-url/open-note?title=My%20Note%3A%20ideas"
        );
        assert_eq!(id.name, "My Note: ideas");
    }

    #[test]
    fn empty_output_degrades_with_explanation() {
        let spec = AdapterSpec {
            app: "Safari".into(),
            script: "…".into(),
            delimiter: Some("\t".into()),
            fields: vec!["url".into(), "title".into()],
            uri: "{url}".into(),
            name: "{title}".into(),
            path: None,
            schemes: vec![],
            needs_permission: true,
            builtin: false,
            user_defined: false,
        };
        let a = ConfiguredAdapter::new(spec, FakeRunner(Ok("   \n".into())));
        match a.capture().unwrap_err() {
            CaptureFailure::AdapterError { explanation } => assert!(explanation.contains("Safari")),
            other => panic!("expected AdapterError, got {other:?}"),
        }
    }

    #[test]
    fn a_stalled_app_times_out_with_an_explanation_naming_it() {
        // T4 + U3 through the main capture path: the adapter's osascript stands in
        // as a real child that never answers; the bounded runner gives up and the
        // user sees which app stalled and what to do about a pending prompt.
        struct StalledApp;
        impl CommandRunner for StalledApp {
            fn run(&self, _p: &str, _a: &[&str]) -> Result<String, String> {
                crate::live_capture::run_bounded(
                    "/bin/sleep",
                    &["5"],
                    std::time::Duration::from_millis(200),
                )
            }
        }
        let spec = AdapterSpec {
            app: "Bear".into(),
            script: "…".into(),
            delimiter: None,
            fields: vec!["title".into()],
            uri: "bear://x-callback-url/open-note?title={title|urlencode}".into(),
            name: "{title}".into(),
            path: None,
            schemes: vec![],
            needs_permission: true,
            builtin: false,
            user_defined: false,
        };
        let start = std::time::Instant::now();
        let err = ConfiguredAdapter::new(spec, StalledApp)
            .capture()
            .unwrap_err();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        let msg = err.explanation();
        assert!(msg.contains("Bear"), "{msg}");
        assert!(msg.contains("didn't respond"), "{msg}");
        assert!(msg.contains("permission prompt"), "{msg}");
    }

    #[test]
    fn defaults_load_and_cover_the_seed_apps() {
        let reg = AdapterRegistry::defaults();
        let apps = reg.apps();
        for expected in ["Safari", "Google Chrome", "Finder", "Bear"] {
            assert!(
                apps.contains(&expected),
                "default adapters must include {expected}; got {apps:?}"
            );
        }
        // case-insensitive match; unknown app -> None (U3 degradation upstream).
        assert!(reg.for_app("safari").is_some());
        assert!(reg.for_app("Slack").is_none());
    }

    #[test]
    fn every_default_spec_template_renders_with_dummy_fields() {
        // Guard the shipped config: each spec's templates reference only declared
        // fields and known transforms (so a typo can't ship a broken adapter).
        let reg = AdapterRegistry::defaults();
        for spec in &reg.specs {
            let dummy: HashMap<String, String> = spec
                .fields
                .iter()
                .map(|f| (f.clone(), "x/y".to_string()))
                .collect();
            render(&spec.uri, &dummy).unwrap_or_else(|e| panic!("{} uri template: {e}", spec.app));
            render(&spec.name, &dummy)
                .unwrap_or_else(|e| panic!("{} name template: {e}", spec.app));
            if let Some(p) = &spec.path {
                render(p, &dummy).unwrap_or_else(|e| panic!("{} path template: {e}", spec.app));
            }
        }
    }

    #[test]
    fn declared_schemes_extracts_literals_not_dynamic_ones() {
        let reg = AdapterRegistry::defaults();
        let schemes = reg.declared_schemes();
        // `things`/`file` from literal templates; `bear` from an explicit declaration
        // (its uri is dynamic — built from the clipboard — so it can't be inferred).
        for s in ["bear", "things", "file"] {
            assert!(
                schemes.contains(&s.to_string()),
                "expected scheme {s}; got {schemes:?}"
            );
        }
        // …but a "{url}" template (browsers) has no literal scheme to extract.
        assert!(!schemes.iter().any(|s| s == "{url}" || s.contains('{')));
    }

    #[test]
    fn user_specs_override_builtins_by_app() {
        let mut reg = AdapterRegistry::defaults();
        let before = reg.apps().len();
        reg.extend_from_json(
            r#"[{"app":"Safari","script":"x","fields":["url"],"uri":"{url}","name":"{url}"},
                {"app":"Slack","script":"x","fields":["u"],"uri":"{u}","name":"{u}"}]"#,
        )
        .unwrap();
        assert!(reg.for_app("Slack").is_some(), "user app added");
        assert_eq!(
            reg.apps()
                .iter()
                .filter(|a| a.eq_ignore_ascii_case("Safari"))
                .count(),
            1,
            "Safari overridden, not duplicated"
        );
        assert_eq!(reg.apps().len(), before + 1);
    }

    // Adapter JSON is TRUSTED LOCAL CONFIG (DECISIONS D14): only the embedded
    // defaults may be built-in. A user entry can never claim `builtin`, because
    // built-ins can't be disabled in the plugin manager.

    #[test]
    fn user_entry_cannot_claim_builtin() {
        let mut reg = AdapterRegistry::defaults();
        reg.extend_from_json(
            r#"[{"app":"Slack","script":"x","fields":["u"],"uri":"{u}","name":"{u}","builtin":true}]"#,
        )
        .unwrap();
        let slack = reg.for_app("Slack").expect("user app added");
        assert!(
            !slack.builtin,
            "a user adapters.json entry must never become builtin (non-disableable)"
        );
    }

    #[test]
    fn user_override_of_a_builtin_is_no_longer_builtin() {
        let mut reg = AdapterRegistry::defaults();
        assert!(
            reg.for_app("Safari").unwrap().builtin,
            "precondition: Safari ships as a built-in"
        );
        // Overriding a built-in by name stays allowed (users may fix a script), and
        // an explicit `"builtin": true` must not keep it locked on.
        reg.extend_from_json(
            r#"[{"app":"safari","script":"user script","fields":["url"],"uri":"{url}","name":"{url}","builtin":true}]"#,
        )
        .unwrap();
        let safari = reg.for_app("Safari").unwrap();
        assert_eq!(safari.script, "user script", "user override took effect");
        assert!(
            !safari.builtin,
            "a user override of a built-in is user-defined, so it must be disableable"
        );
    }

    #[test]
    fn user_override_without_builtin_flag_is_not_builtin() {
        let mut reg = AdapterRegistry::defaults();
        reg.extend_from_json(
            r#"[{"app":"Finder","script":"x","fields":["path"],"uri":"file://{path}","name":"{path}"}]"#,
        )
        .unwrap();
        assert!(!reg.for_app("Finder").unwrap().builtin);
    }

    #[test]
    fn user_entries_are_marked_user_defined_and_defaults_are_not() {
        let mut reg = AdapterRegistry::defaults();
        assert!(
            reg.specs().iter().all(|s| !s.user_defined),
            "embedded defaults are not user-defined"
        );
        // `user_defined` is never read from JSON — only set by the merge.
        reg.extend_from_json(
            r#"[{"app":"Safari","script":"x","fields":["url"],"uri":"{url}","name":"{url}"},
                {"app":"Slack","script":"x","fields":["u"],"uri":"{u}","name":"{u}"}]"#,
        )
        .unwrap();
        assert!(reg.for_app("Safari").unwrap().user_defined, "override");
        assert!(reg.for_app("Slack").unwrap().user_defined, "new app");
        assert!(
            !reg.for_app("Finder").unwrap().user_defined,
            "untouched default"
        );
    }

    #[test]
    fn script_errors_lose_osascript_noise_but_keep_the_message() {
        // Verbatim what a user saw capturing from Finder's Recents view: the
        // offsets, the "execution error:" preamble and the Apple event code are
        // AppleScript internals, not an explanation.
        let raw = "77:82: execution error: Can’t make «class fvtg» of window 1 of application \"Finder\" into type alias. (-1700)";
        assert_eq!(
            clean_script_error(raw),
            "Can’t make «class fvtg» of window 1 of application \"Finder\" into type alias."
        );
        // A script's own `error "…"` arrives wrapped the same way (code -2700).
        assert_eq!(
            clean_script_error("0:180: execution error: “Recents” isn’t a folder on disk. (-2700)"),
            "“Recents” isn’t a folder on disk."
        );
    }

    #[test]
    fn script_errors_without_osascript_framing_are_left_alone() {
        // Only osascript's exact framing is stripped: a parenthesised non-code
        // like "(os error 2)" must survive, as must plain and empty messages.
        for plain in ["osascript timed out", "No such file (os error 2)", ""] {
            assert_eq!(clean_script_error(plain), plain);
        }
    }

    #[test]
    fn shipped_bear_adapter_builds_the_note_link_and_title_from_bearcli() {
        // `bearcli app get-selection --fields id,title` prints "<id>\t<title>".
        // Every earlier title source failed live: Bear's database is in a
        // protected group container ("authorization denied"), and the window's
        // accessibility name is just "Bear". A title containing a tab must not
        // corrupt the id, which is why the id comes first.
        let spec = AdapterRegistry::defaults()
            .for_app("Bear")
            .expect("Bear ships")
            .clone();
        let out = "FC025707-8906-4D1C-A8B2-F44A303471A7\tLocal AI Mini PC Watchlist — Subscription Killers\tpart two";
        let id = ConfiguredAdapter::new(spec, FakeRunner(Ok(out.into())))
            .capture()
            .unwrap();
        assert_eq!(
            id.uri,
            "bear://x-callback-url/open-note?id=FC025707-8906-4D1C-A8B2-F44A303471A7"
        );
        assert_eq!(
            id.name,
            "Local AI Mini PC Watchlist — Subscription Killers\tpart two"
        );
    }

    #[test]
    fn shipped_bear_adapter_leaves_the_clipboard_and_menus_alone() {
        // The old script clicked Bear's Copy Link menu, overwriting whatever the
        // user had copied on every capture, and needed Accessibility to do it.
        let reg = AdapterRegistry::defaults();
        let script = &reg.for_app("Bear").expect("Bear ships").script;
        assert!(
            script.contains("bearcli"),
            "reads the open note via bearcli"
        );
        assert!(script.contains("get-selection"), "asks for the open note");
        for gone in ["clipboard", "click menu item", "System Events", "sqlite3"] {
            assert!(!script.contains(gone), "must not use {gone}");
        }
    }

    #[test]
    fn missing_accessibility_is_explained_as_a_permission_not_an_error() {
        // What a user saw capturing from Bear with Accessibility off: accurate,
        // but it never says the fix is the Accessibility permission — which the
        // setup screen already describes as needed "only for apps like Bear".
        let raw = "118:189: execution error: System Events got an error: osascript is not allowed assistive access. (-1719)";
        let msg = explain_script_error("Bear", raw);
        assert!(msg.contains("Bear"), "names the app: {msg}");
        assert!(msg.contains("Accessibility"), "names the permission: {msg}");
        assert!(
            msg.contains("Permissions"),
            "points at where to fix it: {msg}"
        );
        assert!(
            !msg.contains("osascript"),
            "no AppleScript internals: {msg}"
        );
        assert!(
            !msg.contains("System Events"),
            "no AppleScript internals: {msg}"
        );
    }

    #[test]
    fn other_script_errors_keep_the_cleaned_explanation() {
        assert_eq!(
            explain_script_error(
                "Finder",
                "0:9: execution error: “Recents” isn’t a folder on disk. (-2700)"
            ),
            "Couldn't read the active resource from Finder: “Recents” isn’t a folder on disk."
        );
    }

    #[test]
    fn capture_failure_explanation_is_cleaned() {
        let spec = AdapterRegistry::defaults()
            .for_app("Finder")
            .expect("Finder ships")
            .clone();
        let raw = "0:120: execution error: “Recents” isn’t a folder on disk. Select a file in it, or open a real folder. (-2700)";
        let err = ConfiguredAdapter::new(spec, FakeRunner(Err(raw.into())))
            .capture()
            .unwrap_err();
        match err {
            CaptureFailure::AdapterError { explanation } => assert_eq!(
                explanation,
                "Couldn't read the active resource from Finder: “Recents” isn’t a folder on disk. Select a file in it, or open a real folder."
            ),
            _ => panic!("expected AdapterError"),
        }
    }

    #[test]
    fn builtin_finder_script_handles_views_that_are_not_folders() {
        // Regression guard: Recents, AirDrop, Network and search results have no
        // folder on disk, so `target of front window as alias` fails in them. The
        // shipped script must guard that lookup, fall back to the selection, and
        // not assume a Finder window is open at all.
        let reg = AdapterRegistry::defaults();
        let script = &reg.for_app("Finder").expect("Finder ships").script;
        assert!(script.contains("try"), "target lookup must be guarded");
        assert!(
            script.contains("selection"),
            "must fall back to the selection"
        );
        assert!(
            script.contains("count of Finder windows"),
            "must handle no open window"
        );
    }
}
