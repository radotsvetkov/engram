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

/// What kind of program a skill's bytes are, and therefore which runtime executes it.
///
/// The moat was never WASM: [`module_hash`] and [`verify`] sign arbitrary bytes, so a skill can
/// just as soundly be a *script* (a small Python/JS/Go/shell program — the polyglot, LLM-authorable
/// substrate) as a WASM module. The runtime lives INSIDE the signed manifest, so you cannot swap a
/// Python skill to run under a different interpreter without breaking the signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// A WASM module run in the fuel-bounded, deny-by-default wasmi sandbox. The default and the
    /// only substrate for high-assurance pure-compute transforms (it is exact-byte replayable).
    #[default]
    Wasm,
    /// A source script executed by an interpreter inside the agent's existing shell sandbox
    /// (local / network-isolated `docker run` / ssh). The "small program" skill substrate.
    Process,
}

impl Runtime {
    /// True for the serde default (Wasm). Used by `skip_serializing_if` so that an existing signed
    /// WASM manifest serializes byte-identically (the field is simply absent) and keeps verifying
    /// after the new fields are added — the load-bearing back-compat rule.
    pub fn is_default(&self) -> bool {
        matches!(self, Runtime::Wasm)
    }
}

/// The on-disk file extension for a skill artifact, derived from its runtime + interpreter. Purely
/// cosmetic for readability — the registry globs `v{N}.*`, it does not depend on the extension.
pub fn artifact_ext(runtime: Runtime, interpreter: Option<&str>) -> &'static str {
    match runtime {
        Runtime::Wasm => "wasm",
        Runtime::Process => match interpreter
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("")
        {
            "python3" | "python" => "py",
            "node" | "deno" | "bun" => "js",
            "bash" | "sh" | "zsh" => "sh",
            "ruby" => "rb",
            "go" => "go",
            "gcc" | "cc" | "clang" => "c",
            "perl" => "pl",
            "php" => "php",
            _ => "txt",
        },
    }
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
    /// BLAKE3 hex of the skill's bytes this manifest authorises.
    pub module_hash: String,
    /// Which substrate runs this skill. Defaults to `Wasm` and is OMITTED from the canonical bytes
    /// when default, so existing signed WASM manifests keep verifying unchanged.
    #[serde(default, skip_serializing_if = "Runtime::is_default")]
    pub runtime: Runtime,
    /// For a `Process` skill: the interpreter command (e.g. "python3", "node", "bash", "go run").
    /// Signed, so the interpreter a skill runs under cannot be swapped post-signing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<String>,
    /// A short natural-language cue for auto-selection — when the agent should reach for this skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
}

impl Manifest {
    /// Canonical bytes for signing (serde_json sorts keys deterministically).
    pub fn canonical(&self) -> Result<Vec<u8>, ManifestError> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn grants(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// True if this skill executes code outside the WASM sandbox (a process skill) or holds any
    /// capability that demands a trusted run. Such a skill is refused on a tainted run — the central
    /// dispatch gate only covers *egress* tools, so code-execution needs this explicit check.
    pub fn requires_trust(&self) -> bool {
        self.runtime == Runtime::Process || self.capabilities.iter().any(|c| c.requires_trust())
    }
}

/// A manifest plus its detached signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedSkill {
    pub manifest: Manifest,
    /// Hex Ed25519 signature over BLAKE3(canonical manifest).
    pub sig: String,
}

/// BLAKE3 hex of WASM bytes - the value that goes in `Manifest::module_hash`.
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
            let seed: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| ManifestError::Key)?;
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

    /// Sign an autonomy policy with the SAME key the registry publishes, so an agent's standing
    /// egress grant rides the existing signed-skill trust root (verified with the same public key).
    pub fn sign_policy(
        &self,
        policy: &engram_core::AutonomyPolicy,
    ) -> engram_core::SignedAutonomyPolicy {
        engram_core::sign_policy(policy, &self.signing)
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
            runtime: Runtime::default(),
            interpreter: None,
            when_to_use: None,
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
