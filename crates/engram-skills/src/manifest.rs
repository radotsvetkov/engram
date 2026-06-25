//! Skill manifests and signing.
//!
//! A skill is a WASM module plus a [`Manifest`] describing what it is, which
//! capabilities it needs, and the hash of its bytes. The manifest (which embeds the
//! module hash) is signed with the core's skill key, so a tampered module or an
//! escalated capability set fails verification and never loads. This is what makes a
//! *self-modifying* agent safe: every version is signed, and nothing unsigned runs.

use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::capability::Capability;

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("bad key material")]
    Key,
    #[error("signature invalid")]
    BadSignature,
    #[error("module hash mismatch (bytes do not match manifest)")]
    HashMismatch,
    #[error("randomness unavailable: {0}")]
    Rand(String),
}

fn default_entry() -> String {
    "run".to_string()
}

/// Everything known about a skill except its bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub version: u32,
    /// How the skill is categorised in procedural memory: "thinking",
    /// "problem_solving", "drafting", ...
    pub category: String,
    pub description: String,
    #[serde(default = "default_entry")]
    pub entry: String,
    pub capabilities: Vec<Capability>,
    /// The metric this skill optimises, e.g. "accept_rate".
    pub metric: String,
    /// BLAKE3 hex of the WASM bytes this manifest authorises.
    pub module_hash: String,
}

impl Manifest {
    /// Canonical bytes for signing (serde_json sorts keys deterministically).
    pub fn canonical(&self) -> Result<Vec<u8>, ManifestError> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn grants(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}

/// A manifest plus its detached signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedSkill {
    pub manifest: Manifest,
    /// Hex Ed25519 signature over BLAKE3(canonical manifest).
    pub sig: String,
}

/// BLAKE3 hex of WASM bytes — the value that goes in `Manifest::module_hash`.
pub fn module_hash(wasm: &[u8]) -> String {
    blake3::hash(wasm).to_hex().to_string()
}

/// Holds the skill-signing key. The key lives on disk at `0600`, never inside a
/// skill, mirroring the audit ledger's key handling.
pub struct SkillSigner {
    signing: SigningKey,
    pub verifying: VerifyingKey,
}

impl SkillSigner {
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        let signing = if let Ok(bytes) = std::fs::read(path) {
            let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| ManifestError::Key)?;
            SigningKey::from_bytes(&seed)
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).map_err(|e| ManifestError::Rand(e.to_string()))?;
            write_secret(path, &seed)?;
            SigningKey::from_bytes(&seed)
        };
        let verifying = signing.verifying_key();
        Ok(SkillSigner { signing, verifying })
    }

    /// Sign a manifest, producing a loadable [`SignedSkill`].
    pub fn sign(&self, manifest: &Manifest) -> Result<SignedSkill, ManifestError> {
        let digest = blake3::hash(&manifest.canonical()?);
        let sig = self.signing.sign(digest.as_bytes());
        Ok(SignedSkill {
            manifest: manifest.clone(),
            sig: hex::encode(sig.to_bytes()),
        })
    }
}

/// Verify a signed skill against its bytes and a trusted public key. Checks both that
/// the bytes match the manifest's hash and that the signature is valid.
pub fn verify(signed: &SignedSkill, wasm: &[u8], vk: &VerifyingKey) -> Result<(), ManifestError> {
    if module_hash(wasm) != signed.manifest.module_hash {
        return Err(ManifestError::HashMismatch);
    }
    let digest = blake3::hash(&signed.manifest.canonical()?);
    let sig_bytes: [u8; 64] = hex::decode(&signed.sig)?
        .try_into()
        .map_err(|_| ManifestError::Key)?;
    let sig = Signature::from_bytes(&sig_bytes);
    vk.verify(digest.as_bytes(), &sig)
        .map_err(|_| ManifestError::BadSignature)
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), ManifestError> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.flush()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(hash: &str) -> Manifest {
        Manifest {
            id: "drafter".into(),
            version: 1,
            category: "drafting".into(),
            description: "drafts short messages".into(),
            entry: "run".into(),
            capabilities: vec![Capability::MemoryRead],
            metric: "accept_rate".into(),
            module_hash: hash.into(),
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let signer = SkillSigner::load_or_create(dir.path().join("skill.key")).unwrap();
        let wasm = b"\0asm\x01\0\0\0fake-bytes";
        let m = manifest(&module_hash(wasm));
        let signed = signer.sign(&m).unwrap();
        verify(&signed, wasm, &signer.verifying).unwrap();
    }

    #[test]
    fn tampered_bytes_fail() {
        let dir = tempfile::tempdir().unwrap();
        let signer = SkillSigner::load_or_create(dir.path().join("skill.key")).unwrap();
        let wasm = b"\0asm\x01\0\0\0original";
        let m = manifest(&module_hash(wasm));
        let signed = signer.sign(&m).unwrap();
        assert!(matches!(
            verify(&signed, b"\0asm\x01\0\0\0swapped", &signer.verifying),
            Err(ManifestError::HashMismatch)
        ));
    }

    #[test]
    fn escalated_capabilities_fail() {
        let dir = tempfile::tempdir().unwrap();
        let signer = SkillSigner::load_or_create(dir.path().join("skill.key")).unwrap();
        let wasm = b"\0asm\x01\0\0\0bytes";
        let m = manifest(&module_hash(wasm));
        let mut signed = signer.sign(&m).unwrap();
        // Attacker adds a capability after signing.
        signed.manifest.capabilities.push(Capability::Net);
        assert!(matches!(
            verify(&signed, wasm, &signer.verifying),
            Err(ManifestError::BadSignature)
        ));
    }
}
