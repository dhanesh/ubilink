// Satisfies: O3 (CI signed builds). Thin CLI over `hooky_core::release` so a CI
// pipeline can sign a release bundle with a key from the `HOOKY_SIGNING_KEY` secret
// and self-verify the result. All real logic + tests live in src/release.rs; this
// file is argument plumbing only (kept deliberately untestable-thin).
// See docs/RUNBOOK-ci-signing.md and .github/workflows/release.yml.

use hooky_core::broker::Manifest;
use hooky_core::release::{public_hex, sign_to_json, signing_key_from_hex, verify_json};
use std::process::ExitCode;

fn usage() -> &'static str {
    "hooky-sign — release bundle signer (CI)\n\
     \n\
     Commands:\n\
       sign   --manifest <m.json> --source <src.mjs> --version <N>\n\
              reads HOOKY_SIGNING_KEY (64-hex); writes the signed bundle JSON to stdout\n\
       verify --bundle <bundle.json>\n\
              reads HOOKY_TRUSTED_KEYS (comma-separated 64-hex pubkeys); exit 0 iff valid\n\
       pubkey reads HOOKY_SIGNING_KEY; prints the public key hex to trust\n\
       validate --manifest <m.json>\n\
              checks a third-party manifest against the schema (B3); exit 0 iff valid\n"
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("missing required env var {name}"))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sign") => {
            let key = signing_key_from_hex(&env("HOOKY_SIGNING_KEY")?).map_err(|e| e.message())?;
            let mpath = arg(&args, "--manifest").ok_or("sign needs --manifest <file>")?;
            let spath = arg(&args, "--source").ok_or("sign needs --source <file>")?;
            let version: u32 = arg(&args, "--version").ok_or("sign needs --version <N>")?
                .parse().map_err(|_| "version must be a non-negative integer".to_string())?;
            let manifest: Manifest = serde_json::from_str(
                &std::fs::read_to_string(&mpath).map_err(|e| format!("read {mpath}: {e}"))?,
            ).map_err(|e| format!("parse {mpath}: {e}"))?;
            // B3: refuse to sign a malformed manifest — authors get the error here,
            // not a broken bundle downstream.
            manifest.validate().map_err(|e| format!("invalid manifest: {e}"))?;
            let source = std::fs::read_to_string(&spath).map_err(|e| format!("read {spath}: {e}"))?;
            let json = sign_to_json(&key, manifest, source, version).map_err(|e| e.message())?;
            println!("{json}");
            Ok(())
        }
        Some("verify") => {
            let bpath = arg(&args, "--bundle").ok_or("verify needs --bundle <file>")?;
            let json = std::fs::read_to_string(&bpath).map_err(|e| format!("read {bpath}: {e}"))?;
            let trusted: Vec<String> = env("HOOKY_TRUSTED_KEYS")?
                .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            verify_json(&json, &trusted).map_err(|e| e.message())?;
            eprintln!("[hooky-sign] bundle {bpath} verified against {} trusted key(s) ✓", trusted.len());
            Ok(())
        }
        Some("validate") => {
            // B3: the author-facing check — is this manifest installable as-is?
            let mpath = arg(&args, "--manifest").ok_or("validate needs --manifest <file>")?;
            let manifest: Manifest = serde_json::from_str(
                &std::fs::read_to_string(&mpath).map_err(|e| format!("read {mpath}: {e}"))?,
            ).map_err(|e| format!("parse {mpath}: {e}"))?;
            manifest.validate().map_err(|e| format!("invalid manifest: {e}"))?;
            eprintln!("[hooky-sign] manifest {mpath} is valid ✓ (id={}, {} capability/ies)", manifest.id, manifest.needs.len());
            Ok(())
        }
        Some("pubkey") => {
            let key = signing_key_from_hex(&env("HOOKY_SIGNING_KEY")?).map_err(|e| e.message())?;
            println!("{}", public_hex(&key));
            Ok(())
        }
        _ => Err(usage().to_string()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
