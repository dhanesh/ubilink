// Satisfies: O4. The std-only TcpListener shell over ubilink_registry::serve.
// All logic, the HTTP wire parsing and the bounded accept loop are tested in the
// lib; this reads config, binds a port and hands off. Deployment (TLS, backups,
// real trusted-key set) is in docs/RUNBOOK-registry.md — that hosting is the
// ENV-blocked residual of O4.
//
// Usage:
//   UBILINK_TRUSTED_KEYS=<comma-separated 64-hex pubkeys> \
//   UBILINK_REGISTRY_DATA=<data dir> \
//     ubilink-registry [bind-addr]      # default 127.0.0.1:8787
//
// Without UBILINK_REGISTRY_DATA the index is in memory only: a restart forgets
// every integration and its owner.
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

    let idx = match std::env::var_os("UBILINK_REGISTRY_DATA").filter(|d| !d.is_empty()) {
        Some(dir) => {
            let idx = match RegistryIndex::open(trusted, &dir) {
                Ok(idx) => idx,
                Err(e) => {
                    eprintln!("error: cannot open data dir {}: {e}", dir.to_string_lossy());
                    std::process::exit(1);
                }
            };
            let reserved = idx.reserved_ids();
            eprintln!(
                "[ubilink-registry] data dir {}: loaded {} integration(s), {} reserved (not served){}",
                dir.to_string_lossy(),
                idx.list().len(),
                reserved.len(),
                if reserved.is_empty() {
                    String::new()
                } else {
                    format!(": {}", reserved.join(", "))
                }
            );
            idx
        }
        None => {
            eprintln!(
                "warning: UBILINK_REGISTRY_DATA is not set — the index is in memory only. A \
                 restart forgets every integration AND its owner, so the first trusted \
                 publish after it can claim any id."
            );
            RegistryIndex::new(trusted)
        }
    };

    let listener = TcpListener::bind(&addr)?;
    let cfg = ServerConfig::default();
    eprintln!(
        "[ubilink-registry] listening on {} (max {} connections, body cap {} bytes)",
        listener.local_addr()?,
        cfg.max_connections,
        cfg.limits.max_body
    );
    ubilink_registry::serve(listener, Arc::new(Mutex::new(idx)), cfg)
}
