//! The automation wire protocol (R6, S1): newline-delimited JSON, one request per line,
//! `{"verb": "...", "args": {...}}`, answered by `{"ok": true, "result": ...}` or
//! `{"ok": false, "error": "..."}`.
//!
//! Pure on purpose: everything here runs on the listener thread before a request is
//! allowed anywhere near the GPUI main thread, and is tested headlessly. S1's
//! "accepts only allowlisted verbs" is enforced by construction — a request becomes a
//! typed [`Verb`] or it is refused, and no verb carries code, a command line or a
//! free-form path to execute. There is no eval and no shell-out verb.

use std::path::PathBuf;

use gpui_kit::Keystroke;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::fixtures::Fixture;
use crate::views::{Route, TABLE};

/// Every verb the channel serves, exactly R6's list and nothing else. `parse_request`
/// matches on these literals; `the_allowlist_is_exactly_r6s_verbs` pins the set.
pub const VERBS: [&str; 11] = [
    "open_view",
    "key",
    "state",
    "metrics",
    "trigger_hotkey",
    "resign_key",
    "set_appearance",
    "load_fixture",
    "window_id",
    "a11y",
    "opened",
];

/// Longest request line accepted, in bytes. Requests are tiny; a cap keeps a confused
/// client from growing the listener's buffer without bound.
pub const MAX_LINE: u64 = 64 * 1024;

/// A parsed, validated request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Show the panel on `route` (the tray's "Open" path).
    OpenView(Route),
    /// Keystrokes dispatched, in order, through GPUI's own key dispatch in the panel.
    Key(Vec<Keystroke>),
    State,
    Metrics,
    /// Run the hotkey path for this route, as a real Carbon hotkey event would.
    TriggerHotkey(Route),
    /// The panel loses key status, through the click-away observer's own code.
    ResignKey,
    SetAppearance(Appearance),
    LoadFixture(Fixture, PathBuf),
    WindowId,
    A11y,
    Opened,
}

/// What `set_appearance` asks the theme to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    Dark,
    /// Drop the override and follow macOS again.
    System,
}

impl Appearance {
    pub fn as_str(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
            Appearance::System => "system",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteArgs {
    route: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyArgs {
    keys: String,
}

/// The three hotkeys by the action they trigger, not their chord: a probe asks for
/// "capture", whatever the binding is.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum HotkeyAction {
    Capture,
    Browser,
    Settings,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HotkeyArgs {
    action: HotkeyAction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppearanceArgs {
    mode: Appearance,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureArgs {
    kind: String,
    path: String,
}

/// The route whose `as_str` is `name`, read from the router table so this never grows
/// its own copy of the route names.
pub fn route_named(name: &str) -> Option<Route> {
    TABLE.iter().map(|e| e.route).find(|r| r.as_str() == name)
}

/// `load_fixture`'s kind names. Kept here rather than on [`Fixture`] so the names exist
/// only in automation builds (TN1).
pub const FIXTURE_NAMES: [(Fixture, &str); 5] = [
    (Fixture::Capture, "capture"),
    (Fixture::Permission, "permission"),
    (Fixture::GraphDir, "graph_dir"),
    (Fixture::ExportPath, "export_path"),
    (Fixture::ImportPath, "import_path"),
];

pub fn fixture_name(kind: Fixture) -> &'static str {
    FIXTURE_NAMES
        .iter()
        .find(|(f, _)| *f == kind)
        .map_or("unknown", |(_, n)| n)
}

fn fixture_named(name: &str) -> Option<Fixture> {
    FIXTURE_NAMES
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(f, _)| *f)
}

fn args_as<T: for<'de> Deserialize<'de>>(verb: &str, args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad args for {verb}: {e}"))
}

fn no_args(verb: &str, args: &Map<String, Value>, v: Verb) -> Result<Verb, String> {
    if args.is_empty() {
        Ok(v)
    } else {
        Err(format!("{verb} takes no args"))
    }
}

/// `"down down enter"` → three keystrokes, in GPUI's own syntax (`cmd-c`, `shift-tab`).
pub fn parse_keys(keys: &str) -> Result<Vec<Keystroke>, String> {
    let out: Vec<Keystroke> = keys
        .split_whitespace()
        .map(|k| Keystroke::parse(k).map_err(|e| format!("bad key {k:?}: {e}")))
        .collect::<Result<_, _>>()?;
    if out.is_empty() {
        return Err("key needs at least one keystroke".into());
    }
    Ok(out)
}

/// Parse one request line into a [`Verb`], or the error the client gets back.
pub fn parse_request(line: &str) -> Result<Verb, String> {
    let value: Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))?;
    let Value::Object(mut obj) = value else {
        return Err("bad request: expected a JSON object".into());
    };
    let verb = match obj.remove("verb") {
        Some(Value::String(v)) => v,
        _ => return Err("bad request: missing string field \"verb\"".into()),
    };
    let args = match obj.remove("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err("bad request: \"args\" must be an object".into()),
    };
    if let Some(extra) = obj.keys().next() {
        return Err(format!("bad request: unexpected field {extra:?}"));
    }
    let v = verb.as_str();
    // S1: the allowlist gate. Anything not listed is refused by name before any
    // argument is looked at.
    if !VERBS.contains(&v) {
        return Err(format!("unknown verb: {v}"));
    }
    match v {
        "open_view" => {
            let a: RouteArgs = args_as(v, Value::Object(args))?;
            route_named(&a.route)
                .map(Verb::OpenView)
                .ok_or_else(|| format!("unknown route: {}", a.route))
        }
        "key" => {
            let a: KeyArgs = args_as(v, Value::Object(args))?;
            parse_keys(&a.keys).map(Verb::Key)
        }
        "state" => no_args(v, &args, Verb::State),
        "metrics" => no_args(v, &args, Verb::Metrics),
        "trigger_hotkey" => {
            let a: HotkeyArgs = args_as(v, Value::Object(args))?;
            Ok(Verb::TriggerHotkey(match a.action {
                HotkeyAction::Capture => Route::Capture,
                HotkeyAction::Browser => Route::Browser,
                HotkeyAction::Settings => Route::Settings,
            }))
        }
        "resign_key" => no_args(v, &args, Verb::ResignKey),
        "set_appearance" => {
            let a: AppearanceArgs = args_as(v, Value::Object(args))?;
            Ok(Verb::SetAppearance(a.mode))
        }
        "load_fixture" => {
            let a: FixtureArgs = args_as(v, Value::Object(args))?;
            let kind = fixture_named(&a.kind)
                .ok_or_else(|| format!("unknown fixture kind: {}", a.kind))?;
            let path = PathBuf::from(&a.path);
            // Absolute only: the app's cwd is not the probe's, so a relative path would
            // silently point somewhere else.
            if !path.is_absolute() {
                return Err(format!("fixture path must be absolute: {}", a.path));
            }
            Ok(Verb::LoadFixture(kind, path))
        }
        "window_id" => no_args(v, &args, Verb::WindowId),
        "a11y" => no_args(v, &args, Verb::A11y),
        "opened" => no_args(v, &args, Verb::Opened),
        // Unreachable past the gate above; kept so a verb added to VERBS without a
        // parser is refused rather than guessed at.
        other => Err(format!("unknown verb: {other}")),
    }
}

/// Encode a handler outcome as the response line's JSON (no trailing newline).
pub fn encode(outcome: &Result<Value, String>) -> String {
    let v = match outcome {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(verb: &str, args: Value) -> Result<Verb, String> {
        parse_request(&json!({"verb": verb, "args": args}).to_string())
    }

    /// RED/GREEN anchor for Task 6 (spec R6 verify line). Run with:
    /// `cargo test --locked --manifest-path desktop-gpui/Cargo.toml --features automation automation_rejects_unknown_verb`
    #[test]
    fn automation_rejects_unknown_verb() {
        assert_eq!(
            req("eval", json!({"code": "1+1"})),
            Err("unknown verb: eval".to_string())
        );
        assert_eq!(
            parse_request(r#"{"verb":"sh"}"#),
            Err("unknown verb: sh".to_string())
        );
        // Case matters: the allowlist is exact strings, not a fuzzy match.
        assert_eq!(req("STATE", json!({})), Err("unknown verb: STATE".into()));
        let line = encode(&req("frobnicate", json!({})).map(|_| Value::Null));
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"ok": false, "error": "unknown verb: frobnicate"})
        );
    }

    #[test]
    fn the_allowlist_is_exactly_r6s_verbs() {
        // Every listed verb parses with valid args; nothing else does.
        let valid = [
            ("open_view", json!({"route": "capture"})),
            ("key", json!({"keys": "down"})),
            ("state", json!({})),
            ("metrics", json!({})),
            ("trigger_hotkey", json!({"action": "capture"})),
            ("resign_key", json!({})),
            ("set_appearance", json!({"mode": "dark"})),
            (
                "load_fixture",
                json!({"kind": "capture", "path": "/tmp/f.json"}),
            ),
            ("window_id", json!({})),
            ("a11y", json!({})),
            ("opened", json!({})),
        ];
        let names: Vec<&str> = valid.iter().map(|(v, _)| *v).collect();
        assert_eq!(names, VERBS);
        for (verb, args) in valid {
            assert!(req(verb, args).is_ok(), "{verb} should parse");
        }
    }

    #[test]
    fn malformed_requests_are_refused_with_a_reason() {
        assert!(parse_request("not json")
            .unwrap_err()
            .starts_with("bad request"));
        assert!(parse_request("[1]").unwrap_err().contains("JSON object"));
        assert!(parse_request(r#"{"args":{}}"#)
            .unwrap_err()
            .contains("verb"));
        assert!(parse_request(r#"{"verb":"state","args":[1]}"#)
            .unwrap_err()
            .contains("args"));
        assert!(parse_request(r#"{"verb":"state","cmd":"x"}"#)
            .unwrap_err()
            .contains("unexpected field"));
        // Missing args means none.
        assert_eq!(parse_request(r#"{"verb":"state"}"#), Ok(Verb::State));
    }

    #[test]
    fn argless_verbs_refuse_args() {
        for verb in [
            "state",
            "metrics",
            "resign_key",
            "window_id",
            "a11y",
            "opened",
        ] {
            assert_eq!(
                req(verb, json!({"x": 1})),
                Err(format!("{verb} takes no args"))
            );
        }
    }

    #[test]
    fn open_view_takes_a_router_route() {
        assert_eq!(
            req("open_view", json!({"route": "browser"})),
            Ok(Verb::OpenView(Route::Browser))
        );
        for r in TABLE.iter().map(|e| e.route) {
            assert_eq!(
                req("open_view", json!({"route": r.as_str()})),
                Ok(Verb::OpenView(r))
            );
        }
        assert_eq!(
            req("open_view", json!({"route": "nowhere"})),
            Err("unknown route: nowhere".into())
        );
        assert!(req("open_view", json!({})).is_err());
        assert!(req("open_view", json!({"route": "capture", "x": 1})).is_err());
    }

    #[test]
    fn key_splits_on_whitespace_in_gpui_syntax() {
        let Ok(Verb::Key(keys)) = req("key", json!({"keys": "down down  enter"})) else {
            panic!("key should parse");
        };
        let names: Vec<&str> = keys.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(names, ["down", "down", "enter"]);
        let Ok(Verb::Key(keys)) = req("key", json!({"keys": "cmd-c"})) else {
            panic!("cmd-c should parse");
        };
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "c");
        assert!(keys[0].modifiers.platform, "cmd is the platform modifier");
        assert!(req("key", json!({"keys": "   "})).is_err());
        assert!(req("key", json!({"keys": 3})).is_err());
    }

    #[test]
    fn trigger_hotkey_names_the_action_not_the_chord() {
        assert_eq!(
            req("trigger_hotkey", json!({"action": "capture"})),
            Ok(Verb::TriggerHotkey(Route::Capture))
        );
        assert_eq!(
            req("trigger_hotkey", json!({"action": "browser"})),
            Ok(Verb::TriggerHotkey(Route::Browser))
        );
        assert_eq!(
            req("trigger_hotkey", json!({"action": "settings"})),
            Ok(Verb::TriggerHotkey(Route::Settings))
        );
        // Onboarding has no hotkey.
        assert!(req("trigger_hotkey", json!({"action": "onboarding"})).is_err());
    }

    #[test]
    fn set_appearance_modes() {
        for (m, a) in [
            ("light", Appearance::Light),
            ("dark", Appearance::Dark),
            ("system", Appearance::System),
        ] {
            assert_eq!(
                req("set_appearance", json!({"mode": m})),
                Ok(Verb::SetAppearance(a))
            );
            assert_eq!(a.as_str(), m);
        }
        assert!(req("set_appearance", json!({"mode": "sepia"})).is_err());
    }

    #[test]
    fn load_fixture_kinds_and_absolute_paths() {
        for (f, name) in FIXTURE_NAMES {
            assert_eq!(fixture_name(f), name);
            assert_eq!(
                req("load_fixture", json!({"kind": name, "path": "/fx/a"})),
                Ok(Verb::LoadFixture(f, PathBuf::from("/fx/a")))
            );
        }
        assert_eq!(
            req("load_fixture", json!({"kind": "kernel", "path": "/x"})),
            Err("unknown fixture kind: kernel".into())
        );
        assert_eq!(
            req("load_fixture", json!({"kind": "capture", "path": "rel/x"})),
            Err("fixture path must be absolute: rel/x".into())
        );
    }

    #[test]
    fn responses_have_the_documented_shape() {
        let ok = encode(&Ok(json!({"a": 1})));
        assert_eq!(
            serde_json::from_str::<Value>(&ok).unwrap(),
            json!({"ok": true, "result": {"a": 1}})
        );
        assert!(!ok.contains('\n'), "one response per line");
        let err = encode(&Err("nope".into()));
        assert_eq!(
            serde_json::from_str::<Value>(&err).unwrap(),
            json!({"ok": false, "error": "nope"})
        );
    }
}
