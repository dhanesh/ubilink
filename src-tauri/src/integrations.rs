// Satisfies: S3 (signed integrations — refuse/warn on unsigned/forged/downgraded),
// T4 (integrations versioned, updatable independent of core), O1 (install/update
// independent of core release), RT-10 (signed, versioned, installable), GAP-09
// (signature attack matrix: forged, untrusted, downgrade, unsigned).
// See .manifold/linking-manifesto-app.md
//
// Integration installer. An integration is distributed as a SIGNED bundle: its
// manifest + plugin source + a monotonic version, signed by the publisher's
// ed25519 key. The installer verifies (a) the signer is trusted, (b) the
// signature covers the exact bytes, and (c) the version does not go backwards
// (downgrade protection) — then records it. Installing/updating an integration is
// independent of any core release (O1): the installer mutates only its own state.

use crate::broker::Manifest;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::collections::HashMap;

/// A signed, versioned integration bundle (RT-10).
#[derive(Debug, Clone)]
pub struct SignedBundle {
    pub manifest: Manifest,
    pub source: String,
    pub version: u32,
    /// ed25519 signature over the canonical bytes (64 bytes). Empty = unsigned.
    pub signature: Vec<u8>,
    /// The publisher's ed25519 public key (32 bytes).
    pub signer: Vec<u8>,
}

/// The exact bytes a signature must cover. Any change to id/version/source flips
/// the signature invalid (forgery/tamper detection).
pub fn canonical_message(manifest_id: &str, version: u32, source: &str) -> Vec<u8> {
    format!("hooky-integration\nid={manifest_id}\nv={version}\n{source}").into_bytes()
}

/// Sign a bundle with a publisher key (used by publishers / in tests).
pub fn sign_bundle(key: &SigningKey, manifest: Manifest, source: String, version: u32) -> SignedBundle {
    let msg = canonical_message(&manifest.id, version, &source);
    let sig: Signature = key.sign(&msg);
    SignedBundle {
        manifest,
        source,
        version,
        signature: sig.to_bytes().to_vec(),
        signer: key.verifying_key().to_bytes().to_vec(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// Bundle carries no signature (S3: unsigned is refused).
    Unsigned,
    /// Signer key is not in the trusted set (S3: untrusted publisher).
    UntrustedSigner,
    /// Signature does not cover the bundle bytes (S3: forged/tampered).
    BadSignature,
    /// Attempt to install an older version than is installed (T4/O1: downgrade).
    Downgrade { current: u32, attempted: u32 },
    /// Malformed key/signature bytes.
    Malformed(String),
}

impl InstallError {
    /// User-facing message (the UI warns with this).
    pub fn message(&self) -> String {
        match self {
            InstallError::Unsigned => "This integration is unsigned and was refused.".into(),
            InstallError::UntrustedSigner => "This integration is signed by an untrusted publisher.".into(),
            InstallError::BadSignature => "This integration's signature is invalid (it may have been tampered with).".into(),
            InstallError::Downgrade { current, attempted } => format!("Refused: integration version {attempted} is older than the installed {current}."),
            InstallError::Malformed(e) => format!("Malformed integration bundle: {e}"),
        }
    }
}

/// Installs and updates integrations independently of the core (O1).
pub struct Installer {
    trusted: Vec<VerifyingKey>,
    installed: HashMap<String, u32>, // integration id -> installed version
}

impl Installer {
    pub fn new(trusted_keys: Vec<VerifyingKey>) -> Installer {
        Installer { trusted: trusted_keys, installed: HashMap::new() }
    }

    /// Verify a bundle WITHOUT installing (S3 attack matrix). Returns Ok iff the
    /// bundle is signed by a trusted key over its own bytes and is not a downgrade.
    pub fn verify(&self, bundle: &SignedBundle) -> Result<(), InstallError> {
        if bundle.signature.is_empty() {
            return Err(InstallError::Unsigned);
        }
        let signer_bytes: [u8; 32] = bundle
            .signer
            .as_slice()
            .try_into()
            .map_err(|_| InstallError::Malformed("signer key must be 32 bytes".into()))?;
        let signer = VerifyingKey::from_bytes(&signer_bytes)
            .map_err(|e| InstallError::Malformed(e.to_string()))?;
        // Trusted-publisher check (constant-time-ish set membership on the key bytes).
        if !self.trusted.iter().any(|k| k.to_bytes() == signer.to_bytes()) {
            return Err(InstallError::UntrustedSigner);
        }
        let sig_bytes: [u8; 64] = bundle
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| InstallError::Malformed("signature must be 64 bytes".into()))?;
        let sig = Signature::from_bytes(&sig_bytes);
        let msg = canonical_message(&bundle.manifest.id, bundle.version, &bundle.source);
        signer.verify(&msg, &sig).map_err(|_| InstallError::BadSignature)?;
        // Downgrade protection (T4/O1).
        if let Some(&current) = self.installed.get(&bundle.manifest.id) {
            if bundle.version < current {
                return Err(InstallError::Downgrade { current, attempted: bundle.version });
            }
        }
        Ok(())
    }

    /// Verify then record the installed version. Independent of core (O1).
    pub fn install(&mut self, bundle: &SignedBundle) -> Result<(), InstallError> {
        self.verify(bundle)?;
        self.installed.insert(bundle.manifest.id.clone(), bundle.version);
        Ok(())
    }

    pub fn installed_version(&self, id: &str) -> Option<u32> {
        self.installed.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
    fn manifest(id: &str) -> Manifest {
        Manifest { id: id.into(), name: "X".into(), app: "Obsidian".into(), version: "1".into(), needs: vec![], signature: None }
    }

    #[test]
    fn trusted_signed_bundle_installs() {
        let k = key(1);
        let mut inst = Installer::new(vec![k.verifying_key()]);
        let b = sign_bundle(&k, manifest("a.b"), "export function run(){}".into(), 1);
        assert!(inst.install(&b).is_ok());
        assert_eq!(inst.installed_version("a.b"), Some(1));
    }

    #[test]
    fn forged_bundle_is_rejected() {
        // GAP-09: sign, then tamper the source -> signature no longer covers it.
        let k = key(1);
        let inst = Installer::new(vec![k.verifying_key()]);
        let mut b = sign_bundle(&k, manifest("a.b"), "good()".into(), 1);
        b.source = "evil()".into(); // tamper after signing
        assert_eq!(inst.verify(&b), Err(InstallError::BadSignature));
    }

    #[test]
    fn untrusted_signer_is_rejected() {
        // GAP-09: validly signed, but by a key the installer does not trust.
        let publisher = key(1);
        let attacker = key(9);
        let inst = Installer::new(vec![publisher.verifying_key()]);
        let b = sign_bundle(&attacker, manifest("a.b"), "run()".into(), 1);
        assert_eq!(inst.verify(&b), Err(InstallError::UntrustedSigner));
    }

    #[test]
    fn unsigned_bundle_is_rejected() {
        // GAP-09: empty signature must be refused, not silently accepted.
        let k = key(1);
        let inst = Installer::new(vec![k.verifying_key()]);
        let mut b = sign_bundle(&k, manifest("a.b"), "run()".into(), 1);
        b.signature.clear();
        assert_eq!(inst.verify(&b), Err(InstallError::Unsigned));
    }

    #[test]
    fn downgrade_is_rejected_but_upgrade_allowed() {
        // T4/O1: versions move forward independently of core; never backward.
        let k = key(1);
        let mut inst = Installer::new(vec![k.verifying_key()]);
        inst.install(&sign_bundle(&k, manifest("a.b"), "v2".into(), 2)).unwrap();
        // downgrade to v1 refused
        let v1 = sign_bundle(&k, manifest("a.b"), "v1".into(), 1);
        assert_eq!(inst.verify(&v1), Err(InstallError::Downgrade { current: 2, attempted: 1 }));
        // upgrade to v3 allowed
        let v3 = sign_bundle(&k, manifest("a.b"), "v3".into(), 3);
        assert!(inst.install(&v3).is_ok());
        assert_eq!(inst.installed_version("a.b"), Some(3));
    }
}
