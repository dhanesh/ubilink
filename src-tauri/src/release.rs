// Satisfies: O3 (CI signed builds — release artifacts are signed by a pipeline key,
// not a dev laptop, and the pipeline self-verifies), S3/RT-10 (the produced bundle
// validates under the same ed25519 trust model the Installer enforces), O1/T4
// (versioned, downgrade-protected). See .manifold/linking-manifesto-app.md
//
// The release-signing seam for CI. The CRYPTO is R3's proven core (`sign_bundle` +
// `Installer::verify`, which already passed the GAP-09 attack matrix). This module
// adds only what a pipeline needs: a JSON wire format for a signed bundle and pure
// sign/verify entry points keyed by hex strings (so a key can come from a CI secret
// env var). `bin/ubilink-sign.rs` is a thin CLI over these functions.
//
// ENV boundary: every function here is unit-tested headlessly (sign->verify
// roundtrip, tamper rejection, untrusted-key rejection, malformed input). The ONLY
// part that needs the real environment is GitHub Actions executing the CLI with the
// org's `UBILINK_SIGNING_KEY` secret — see docs/RUNBOOK-ci-signing.md and
// .github/workflows/release.yml.

use crate::broker::Manifest;
use crate::integrations::{sign_bundle, InstallError, Installer, SignedBundle};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// JSON-serializable signed bundle (the artifact CI produces and uploads). Binary
/// fields are hex so the artifact is a plain text file, diffable and secret-free
/// (it carries only the PUBLIC signer key, never the signing key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireBundle {
    pub manifest: Manifest,
    pub source: String,
    pub version: u32,
    pub signature_hex: String,
    pub signer_hex: String,
}

impl WireBundle {
    fn from_signed(b: &SignedBundle) -> WireBundle {
        WireBundle {
            manifest: b.manifest.clone(),
            source: b.source.clone(),
            version: b.version,
            signature_hex: to_hex(&b.signature),
            signer_hex: to_hex(&b.signer),
        }
    }
    fn into_signed(self) -> Result<SignedBundle, SignError> {
        Ok(SignedBundle {
            manifest: self.manifest,
            source: self.source,
            version: self.version,
            signature: from_hex(&self.signature_hex)?,
            signer: from_hex(&self.signer_hex)?,
        })
    }
}

/// Anything that can go wrong building or checking a release artifact. Distinct
/// from `InstallError` (which is the on-device install verdict).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// A hex field was not valid hex / wrong length.
    BadHex(String),
    /// A signing key was not 32 bytes.
    BadKey(String),
    /// JSON (de)serialization failed.
    Json(String),
    /// The produced/loaded artifact failed verification (carries the install verdict).
    Verify(InstallError),
}

impl SignError {
    pub fn message(&self) -> String {
        match self {
            SignError::BadHex(e) => format!("invalid hex: {e}"),
            SignError::BadKey(e) => format!("invalid signing key: {e}"),
            SignError::Json(e) => format!("bundle JSON error: {e}"),
            SignError::Verify(v) => v.message(),
        }
    }
}

/// Decode a 32-byte ed25519 signing key from hex (the CI secret `UBILINK_SIGNING_KEY`).
pub fn signing_key_from_hex(hex: &str) -> Result<SigningKey, SignError> {
    let bytes = from_hex(hex.trim())?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| SignError::BadKey(format!("expected 32 bytes, got {}", bytes.len())))?;
    Ok(SigningKey::from_bytes(&arr))
}

/// The public (verifying) key hex for a signing key — what the app trusts.
pub fn public_hex(key: &SigningKey) -> String {
    to_hex(&key.verifying_key().to_bytes())
}

/// Sign a release bundle and emit the JSON artifact (the CI `sign` step).
pub fn sign_to_json(
    key: &SigningKey,
    manifest: Manifest,
    source: String,
    version: u32,
) -> Result<String, SignError> {
    let signed = sign_bundle(key, manifest, source, version);
    serde_json::to_string_pretty(&WireBundle::from_signed(&signed))
        .map_err(|e| SignError::Json(e.to_string()))
}

/// Verify a JSON artifact against a set of trusted public-key hexes (the CI
/// self-verify step, and the model an on-device installer uses). Ok iff signed by a
/// trusted key over its own bytes.
pub fn verify_json(json: &str, trusted_pub_hex: &[String]) -> Result<(), SignError> {
    let wire: WireBundle =
        serde_json::from_str(json).map_err(|e| SignError::Json(e.to_string()))?;
    let signed = wire.into_signed()?;
    let mut trusted = Vec::with_capacity(trusted_pub_hex.len());
    for h in trusted_pub_hex {
        let bytes = from_hex(h.trim())?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| SignError::BadKey("trusted key must be 32 bytes".into()))?;
        trusted.push(VerifyingKey::from_bytes(&arr).map_err(|e| SignError::BadKey(e.to_string()))?);
    }
    Installer::new(trusted)
        .verify(&signed)
        .map_err(SignError::Verify)
}

// ── dependency-free hex (S2: no new crates) ──
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

fn from_hex(s: &str) -> Result<Vec<u8>, SignError> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(SignError::BadHex("odd length".into()));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char)
            .to_digit(16)
            .ok_or_else(|| SignError::BadHex(format!("non-hex char '{}'", bytes[i] as char)))?;
        let lo = (bytes[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| SignError::BadHex(format!("non-hex char '{}'", bytes[i + 1] as char)))?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str) -> Manifest {
        Manifest {
            id: id.into(),
            name: "Demo".into(),
            app: "Obsidian".into(),
            version: "1".into(),
            needs: vec![],
            signature: None,
        }
    }
    // A deterministic 32-byte key as hex (test-only; real keys come from a secret).
    fn key_hex(seed: u8) -> String {
        to_hex(&[seed; 32])
    }

    #[test]
    fn hex_roundtrips_all_byte_values() {
        let bytes: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn from_hex_rejects_garbage() {
        assert!(matches!(from_hex("zz"), Err(SignError::BadHex(_))));
        assert!(matches!(from_hex("abc"), Err(SignError::BadHex(_))));
    }

    #[test]
    fn ci_sign_then_self_verify_roundtrips() {
        // O3 happy path: the pipeline signs with the secret key and the self-verify
        // step (and any on-device installer trusting that pubkey) accepts it.
        let key = signing_key_from_hex(&key_hex(7)).unwrap();
        let pub_hex = public_hex(&key);
        let json = sign_to_json(
            &key,
            manifest("ubilink.obsidian"),
            "export function run(){}".into(),
            3,
        )
        .unwrap();
        assert!(verify_json(&json, &[pub_hex]).is_ok());
    }

    #[test]
    fn tampered_artifact_fails_verify() {
        // GAP-09 over the wire format: editing the shipped source after signing is
        // caught (the signature covers the manifest, version and source).
        let key = signing_key_from_hex(&key_hex(7)).unwrap();
        let pub_hex = public_hex(&key);
        let json = sign_to_json(&key, manifest("ubilink.obsidian"), "good()".into(), 1).unwrap();
        let tampered = json.replace("good()", "evil()");
        assert_ne!(
            json, tampered,
            "test setup: source must appear in the artifact"
        );
        assert_eq!(
            verify_json(&tampered, &[pub_hex]),
            Err(SignError::Verify(InstallError::BadSignature))
        );
    }

    #[test]
    fn artifact_with_widened_capabilities_fails_verify() {
        // A registry, mirror or cache editing `needs` in the shipped JSON must be
        // caught — those strings become the plugin's broker grants.
        let key = signing_key_from_hex(&key_hex(7)).unwrap();
        let pub_hex = public_hex(&key);
        let mut m = manifest("ubilink.obsidian");
        m.needs = vec!["open:obsidian://".into()];
        let json = sign_to_json(&key, m, "run()".into(), 1).unwrap();
        let tampered = json.replace("open:obsidian://", "open:file://");
        assert_ne!(
            json, tampered,
            "test setup: needs must appear in the artifact"
        );
        assert_eq!(
            verify_json(&tampered, &[pub_hex]),
            Err(SignError::Verify(InstallError::BadSignature))
        );
    }

    #[test]
    fn artifact_signed_by_untrusted_key_is_rejected() {
        // An attacker who builds a validly-signed artifact with THEIR key is refused
        // because the app trusts only the release pubkey.
        let attacker = signing_key_from_hex(&key_hex(9)).unwrap();
        let release_pub = public_hex(&signing_key_from_hex(&key_hex(1)).unwrap());
        let json =
            sign_to_json(&attacker, manifest("ubilink.obsidian"), "run()".into(), 1).unwrap();
        assert_eq!(
            verify_json(&json, &[release_pub]),
            Err(SignError::Verify(InstallError::UntrustedSigner))
        );
    }

    #[test]
    fn signing_key_from_hex_rejects_wrong_length() {
        assert!(matches!(
            signing_key_from_hex("00ff"),
            Err(SignError::BadKey(_))
        ));
    }

    #[test]
    fn verify_json_reports_malformed_json() {
        assert!(matches!(
            verify_json("not json", &[]),
            Err(SignError::Json(_))
        ));
    }
}
