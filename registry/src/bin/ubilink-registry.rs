// Satisfies: O4. The std-only TcpListener shell over ubilink_registry::serve.
// All logic, the HTTP wire parsing and the bounded accept loop are tested in the
// lib; this reads config, binds a port and hands off. Deployment (TLS,
// persistence, real trusted-key set) is in docs/RUNBOOK-registry.md — that
// hosting is the ENV-blocked residual of O4.
//
// Usage:
//   UBILINK_TRUSTED_KEYS=<comma-separated 64-hex pubkeys> \
//     ubilink-registry [bind-addr]      # default 127.0.0.1:8787
use std::io;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use ubilink_registry::{RegistryIndex, ServerConfig};

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
    let cfg = ServerConfig::default();
    eprintln!(
        "[ubilink-registry] listening on {addr} (max {} connections, body cap {} bytes)",
        cfg.max_connections, cfg.limits.max_body
    );
    ubilink_registry::serve(listener, idx, cfg)
}
