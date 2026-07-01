// Satisfies: O4. The std-only TcpListener shell over ubilink_registry::handle_stream.
// All logic + the HTTP wire parsing are tested in the lib; this binds a port and
// loops. Deployment (TLS, persistence, real trusted-key set) is in
// docs/RUNBOOK-registry.md — that hosting is the ENV-blocked residual of O4.
//
// Usage:
//   UBILINK_TRUSTED_KEYS=<comma-separated 64-hex pubkeys> \
//     ubilink-registry [bind-addr]      # default 127.0.0.1:8787
use std::io;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use ubilink_registry::RegistryIndex;

fn main() -> io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let trusted: Vec<String> = std::env::var("UBILINK_TRUSTED_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if trusted.is_empty() {
        eprintln!("warning: UBILINK_TRUSTED_KEYS is empty — every publish will be refused (no trusted publisher).");
    }

    let idx = Arc::new(Mutex::new(RegistryIndex::new(trusted)));
    let listener = TcpListener::bind(&addr)?;
    eprintln!("[ubilink-registry] listening on {addr}");

    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let idx = Arc::clone(&idx);
        // One thread per connection — fine for a low-traffic registry; the handler
        // serves exactly one request then the connection closes (Connection: close).
        std::thread::spawn(move || {
            let mut guard = match idx.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(), // a poisoned lock still serves reads safely
            };
            if let Err(e) = ubilink_registry::handle_stream(&mut stream, &mut guard) {
                eprintln!("connection error: {e}");
            }
        });
    }
    Ok(())
}
