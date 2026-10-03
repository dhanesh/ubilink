//! Single instance: one ubilink-next per user. A second launch connects to the first's
//! local socket, asks it to show the panel, and exits.
//!
//! LaunchServices already folds a second `open ubilink-next.app` into a reopen event
//! (handled by `Application::on_reopen` in main.rs); this socket covers what it does not:
//! running the binary directly, or `open -n`. It is deliberately a different socket from
//! the automation channel (R6: "separate from the single-instance socket"), and carries
//! one fixed word, so it never becomes a control surface.
//!
//! The socket is a file in `$TMPDIR` (per user on macOS) — interprocess's namespaced
//! sockets are Linux/Windows only. `UBILINK_INSTANCE_SOCKET` overrides the path so a test
//! instance can run beside the user's own without signalling it.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions, Stream};

pub const SOCKET_ENV: &str = "UBILINK_INSTANCE_SOCKET";
const SHOW: &str = "show";

pub fn socket_path() -> PathBuf {
    std::env::var_os(SOCKET_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("org.ubilink.next.instance.sock"))
}

/// The outcome of claiming the instance.
pub enum Claim {
    /// Another instance is running and was told to show its panel: exit now.
    Signalled,
    /// This is the only instance; the listener thread forwards "show" requests to `tx`.
    Primary,
    /// The socket could not be created; run anyway (one instance is more important than
    /// a perfect guard) and say so.
    Unguarded(String),
}

/// Try to signal a running instance; if none answers, become the primary and start a
/// listener thread that sends `()` on `tx` for each "show" request.
pub fn claim(tx: Sender<()>) -> Claim {
    let path = socket_path();
    if let Ok(name) = path.as_os_str().to_fs_name::<GenericFilePath>() {
        if let Ok(mut conn) = Stream::connect(name) {
            if conn.write_all(format!("{SHOW}\n").as_bytes()).is_ok() {
                return Claim::Signalled;
            }
        }
    }
    let name = match path.as_os_str().to_fs_name::<GenericFilePath>() {
        Ok(n) => n,
        Err(e) => return Claim::Unguarded(format!("socket name {}: {e}", path.display())),
    };
    // try_overwrite: a crashed instance leaves a socket file nobody listens on; nobody
    // answered the connect above, so replacing it is safe.
    let listener = match ListenerOptions::new()
        .name(name)
        .try_overwrite(true)
        .create_sync()
    {
        Ok(l) => l,
        Err(e) => return Claim::Unguarded(format!("listen on {}: {e}", path.display())),
    };
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for conn in listener.incoming().filter_map(Result::ok) {
                let mut line = String::new();
                if BufReader::new(conn).read_line(&mut line).is_ok() && line.trim() == SHOW {
                    // The receiver is drained by the GPUI pump; if the app is gone,
                    // stop listening.
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            }
        })
        .map_or_else(
            |e| Claim::Unguarded(format!("listener thread: {e}")),
            |_| Claim::Primary,
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn second_claim_signals_the_first() {
        let dir = std::env::temp_dir().join(format!("ubn-si-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("i.sock");
        // The only test that sets SOCKET_ENV, and the only reader is `claim` below.
        std::env::set_var(SOCKET_ENV, &path);
        let (tx, rx) = mpsc::channel();
        assert!(matches!(claim(tx), Claim::Primary));
        let (tx2, _rx2) = mpsc::channel();
        assert!(matches!(claim(tx2), Claim::Signalled));
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "first got show"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
