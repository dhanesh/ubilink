//! The automation channel (R6): a debug-only control and observation surface that lets
//! an agent drive the real app and read what it shows, compiled ONLY with the
//! `automation` cargo feature (S1). main.rs gates the `mod automation;` line itself,
//! so a release build contains none of this — no socket path, no verb names, no
//! marker — and `scripts/probe.py release-clean` proves it on the binary (TN1).
//!
//! Protocol: newline-delimited JSON over a uid-checked 0600 Unix socket (`socket.rs`,
//! S1), separate from the single-instance socket (S2). Requests parse into a typed
//! allowlisted [`protocol::Verb`] on the listener thread; there is no eval and no
//! arbitrary command.
//!
//! Threading: the listener thread and its per-connection threads never touch AppKit or
//! gpui. A parsed request travels to the GPUI main thread through the shell's own
//! `Inbox` (mpsc + `wake::Signal`) as `ShellEvent::Automation`, so the event-driven
//! pump stays the single way in (no poll, Phase 1 O1), and the answer comes back on a
//! per-request reply channel. `a11y` is the one verb that replies later, from a GPUI
//! foreground task, after the activated AccessKit tree has rendered a frame (D6, TN5).
//!
//! Side effects (O4): opens land in the sink (`crate::opener`, `opened`), file sources
//! come from fixtures (`crate::fixtures`, `load_fixture`), and `trigger_hotkey` /
//! `resign_key` run the shell's own code paths without a real key event, CGEvent or
//! Accessibility permission.

pub mod protocol;
pub mod socket;
pub mod store;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::{AnyWindowHandle, App};
use objc2::MainThreadMarker;
use serde_json::{json, Map, Value};

use crate::shell::{self, native, Command, Inbox, ShellEvent};
use crate::views;
use protocol::{Appearance, Verb};

/// A token that exists only in automation builds. `probe.py release-clean` fails a
/// release binary that contains it and requires an automation binary to contain it,
/// which proves the check can see what it looks for (TN1). Printed at startup so the
/// optimiser keeps it.
pub const MARKER: &str = "UBILINK_AUTOMATION_MARKER";

/// How long a connection waits for the main thread to answer one request.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// `a11y`: how long to wait for an activated tree to render, in 16 ms steps.
const A11Y_TICK: Duration = Duration::from_millis(16);
const A11Y_TICKS: u32 = 125;

type Outcome = Result<Value, String>;

/// One request in flight to the main thread, with the channel its answer goes back on.
#[derive(Clone, Debug)]
pub struct Call {
    id: u64,
    pub verb: Verb,
    reply: Sender<Outcome>,
}

// `ShellEvent` is `PartialEq + Eq` for its tests; a call is identified by its id.
impl PartialEq for Call {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for Call {}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Bind the socket and start the listener thread. Called from main.rs before GPUI
/// starts (the inbox exists already; requests queue until the pump runs). A failure is
/// logged and the app runs without the channel — the probe then fails to connect,
/// loudly, rather than the app refusing to start.
pub fn start(inbox: Inbox) {
    match listen(inbox) {
        // The verb list is printed as data, not only matched on: LLVM compiles short
        // string matches into integer compares, so without this the verb names would
        // not appear in the binary and `release-clean` could not prove it sees them.
        Ok(path) => eprintln!(
            "[ubilink-next] {MARKER}: automation channel listening on {} (verbs: {})",
            path.display(),
            protocol::VERBS.join(" ")
        ),
        Err(e) => eprintln!("[ubilink-next] automation channel unavailable: {e}"),
    }
}

fn listen(inbox: Inbox) -> Result<std::path::PathBuf, String> {
    let uid = socket::current_uid();
    let instance = socket::instance_name(std::env::var(socket::INSTANCE_ENV).ok().as_deref())?;
    let path = socket::socket_path(&std::env::temp_dir(), uid, &instance)?;
    let listener = socket::bind(&path, uid).map_err(|e| format!("{}: {e}", path.display()))?;
    std::thread::Builder::new()
        .name("automation".into())
        .spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                // S1: the uid check comes before reading a single byte.
                let peer = socket::peer_uid(&stream);
                if !socket::peer_allowed(uid, peer) {
                    eprintln!("[ubilink-next] automation: refused peer uid {peer:?}");
                    continue;
                }
                let inbox = inbox.clone();
                let _ = std::thread::Builder::new()
                    .name("automation-conn".into())
                    .spawn(move || serve(stream, &inbox));
            }
        })
        .map_err(|e| format!("listener thread: {e}"))?;
    Ok(path)
}

/// One connection: read a line, answer a line, until the client hangs up.
fn serve(stream: UnixStream, inbox: &Inbox) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut writer = stream;
    loop {
        let mut line = String::new();
        let n = match (&mut reader).take(protocol::MAX_LINE).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if n as u64 == protocol::MAX_LINE && !line.ends_with('\n') {
            let _ = writeln!(
                writer,
                "{}",
                protocol::encode(&Err("request too long".into()))
            );
            return;
        }
        if line.trim().is_empty() {
            continue;
        }
        let outcome = protocol::parse_request(&line).and_then(|verb| call(inbox, verb));
        if writeln!(writer, "{}", protocol::encode(&outcome)).is_err() {
            return;
        }
    }
}

/// Hand a verb to the main thread and wait for its answer.
fn call(inbox: &Inbox, verb: Verb) -> Outcome {
    let (reply, answer) = mpsc::channel();
    inbox.post(ShellEvent::Automation(Call {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        verb,
        reply,
    }));
    answer
        .recv_timeout(REPLY_TIMEOUT)
        .unwrap_or_else(|e| Err(format!("main thread did not answer: {e}")))
}

/// Main thread: run one call (from `shell::handle_event`).
pub fn handle(call: Call, cx: &mut App) {
    let Call { verb, reply, .. } = call;
    match verb {
        Verb::A11y => a11y(reply, cx),
        verb => {
            // A dropped receiver means the client gave up; nothing to do.
            let _ = reply.send(run(verb, cx));
        }
    }
}

fn run(verb: Verb, cx: &mut App) -> Outcome {
    match verb {
        Verb::OpenView(route) => {
            shell::execute(Command::Open(route), cx);
            Ok(state_json(cx))
        }
        Verb::Key(keys) => {
            let (handle, _, _) = shell::panel(cx).ok_or("no panel open")?;
            handle
                .update(cx, |_, window, cx| {
                    keys.into_iter()
                        .map(|k| {
                            let name = k.to_string();
                            let handled = window.dispatch_keystroke(k, cx);
                            json!({"key": name, "handled": handled})
                        })
                        .collect::<Vec<_>>()
                })
                .map(Value::from)
                .map_err(|e| format!("panel window gone: {e}"))
        }
        Verb::State => Ok(state_json(cx)),
        Verb::Metrics => {
            let (capacity, samples) = store::metrics();
            Ok(json!({"capacity": capacity, "samples": samples}))
        }
        Verb::TriggerHotkey(route) => {
            let id = shell::hotkey_id_for(route, cx)
                .ok_or_else(|| format!("no hotkey bound to {}", route.as_str()))?;
            // Measured from here: R5's hotkey-to-first-frame clock starts when the
            // hotkey path is entered, exactly where a real Carbon event would enter it.
            store::mark_hotkey(route, Instant::now());
            shell::handle_event(ShellEvent::HotKey { id, pressed: true }, cx);
            Ok(state_json(cx))
        }
        Verb::ResignKey => {
            let (handle, view, route) = shell::panel(cx).ok_or("no panel open")?;
            let closed = handle
                .update(cx, |_, window, cx| {
                    view.update(cx, |v, cx| v.simulate_resign_key(window, cx))
                })
                .map_err(|e| format!("panel window gone: {e}"))?;
            Ok(json!({"route": route.as_str(), "closed": closed, "state": state_json(cx)}))
        }
        Verb::SetAppearance(mode) => set_appearance(mode, cx),
        Verb::LoadFixture(kind, path) => {
            store::set_fixture(kind, path);
            Ok(fixtures_json())
        }
        Verb::WindowId => {
            // The shell is the authority on whether a panel is open: AppKit can still
            // show a just-removed panel for a run-loop turn (see native::visible_panel).
            shell::panel(cx).ok_or("no panel open")?;
            let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
            native::panel_window_number(mtm)
                .map(|id| json!({"window_id": id}))
                .ok_or_else(|| "no panel open".into())
        }
        Verb::Opened => Ok(json!(store::opened())),
        Verb::A11y => unreachable!("a11y replies asynchronously from `handle`"),
    }
}

/// `state`: the shell's live state, the current route's view-state (D3: the view's
/// own serialisable model, never GPUI's element tree) and `window_verified` lifted to
/// the top level, since it is what most probes gate on.
fn state_json(cx: &mut App) -> Value {
    let s = shell::state(cx);
    let route = shell::panel(cx).map(|(_, _, r)| r);
    let view = route.map(|r| views::state_for(r).to_json());
    json!({
        "window_verified": s.window_verified,
        "route": route.map(|r| r.as_str()),
        "view": view,
        "appearance": {
            "override": store::appearance().as_str(),
            "theme": if cx.theme().mode.is_dark() { "dark" } else { "light" },
        },
        "fixtures": fixtures_json(),
        // O1: the probe checks each instance resolved its own data dir.
        "data_dir": crate::paths::data_dir().display().to_string(),
        "shell": s,
    })
}

fn fixtures_json() -> Value {
    let map: Map<String, Value> = store::fixtures()
        .into_iter()
        .map(|(k, p)| {
            (
                protocol::fixture_name(k).to_string(),
                Value::from(p.display().to_string()),
            )
        })
        .collect();
    Value::Object(map)
}

/// Override what the theme follows: NSApp's appearance (so the panel's blur and every
/// AppKit-drawn part agree) and the gpui-component theme mode; `system` drops both
/// overrides and re-syncs with macOS.
fn set_appearance(mode: Appearance, cx: &mut App) -> Outcome {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let name = match mode {
        Appearance::Light => Some("NSAppearanceNameAqua"),
        Appearance::Dark => Some("NSAppearanceNameDarkAqua"),
        Appearance::System => None,
    };
    native::set_app_appearance(mtm, name);
    match mode {
        Appearance::Light => Theme::change(ThemeMode::Light, None, cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, None, cx),
        Appearance::System => crate::theme::sync(cx),
    }
    store::set_appearance(mode);
    cx.refresh_windows();
    Ok(state_json(cx)["appearance"].clone())
}

/// `a11y` (D6, TN5): activate GPUI's AccessKit tree in-process and return it.
///
/// GPUI builds the tree only once an assistive client activates the window's
/// accesskit_macos adapter, which happens the first time anything asks the content
/// view for `accessibilityChildren`. Sending that message from inside the process
/// activates it with no VoiceOver, no Accessibility (TCC) grant and no second
/// process. GPUI's activation hook then refreshes the window; the next frame builds
/// the tree, and `Window::debug_a11y_tree_json` serialises it. We wait for a frame
/// newer than the one current at the call, so a route switch is never answered with
/// the previous view's tree.
fn a11y(reply: Sender<Outcome>, cx: &mut App) {
    let Some((handle, _, _)) = shell::panel(cx) else {
        let _ = reply.send(Err("no panel open".into()));
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        let _ = reply.send(Err("not on the main thread".into()));
        return;
    };
    let poked = native::poke_accessibility(mtm);
    let start = handle.update(cx, |_, window, _| {
        window.refresh();
        (
            window.is_a11y_enabled(),
            frame_number(window.debug_a11y_tree_json().as_deref()),
        )
    });
    let (enabled, start_frame) = match start {
        Ok(v) => v,
        Err(e) => {
            let _ = reply.send(Err(format!("panel window gone: {e}")));
            return;
        }
    };
    cx.spawn(async move |cx| {
        let outcome = wait_for_tree(handle, start_frame, cx).await;
        let _ =
            reply.send(outcome.map_err(|why| {
                format!("{why} (a11y enabled={enabled}, contentView poked={poked})")
            }));
    })
    .detach();
}

async fn wait_for_tree(
    handle: AnyWindowHandle,
    start_frame: Option<u64>,
    cx: &mut gpui_kit::AsyncApp,
) -> Outcome {
    let mut last = (false, None);
    for _ in 0..A11Y_TICKS {
        cx.background_executor().timer(A11Y_TICK).await;
        let read = cx.update(|cx| {
            handle.update(cx, |_, window, _| {
                (window.is_a11y_active(), window.debug_a11y_tree_json())
            })
        });
        let (active, json) = read.map_err(|e| format!("panel closed while waiting: {e}"))?;
        let frame = frame_number(json.as_deref());
        if active && frame.is_some() && frame > start_frame {
            let json = json.unwrap_or_default();
            return serde_json::from_str(&json).map_err(|e| format!("bad tree JSON: {e}"));
        }
        last = (active, frame);
    }
    Err(format!(
        "no accessibility frame within {} ms: active={}, last frame={:?}, start frame={:?}",
        A11Y_TICK.as_millis() * u128::from(A11Y_TICKS),
        last.0,
        last.1,
        start_frame
    ))
}

/// `frame.frame_number` from a `debug_a11y_tree_json` dump.
fn frame_number(json: Option<&str>) -> Option<u64> {
    let v: Value = serde_json::from_str(json?).ok()?;
    v["frame"]["frame_number"].as_u64()
}

#[cfg(test)]
mod tests {
    use super::frame_number;

    #[test]
    fn frame_number_reads_the_dump_header() {
        assert_eq!(frame_number(None), None);
        assert_eq!(frame_number(Some("not json")), None);
        assert_eq!(
            frame_number(Some(r#"{"frame":{"frame_number":7},"nodes":{}}"#)),
            Some(7)
        );
        assert_eq!(frame_number(Some(r#"{"frame":null}"#)), None);
    }
}
