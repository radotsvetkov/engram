//! The skill registry — procedural memory on disk.
//!
//! Skills live under `skills/<id>/`: each version is a `v<N>.wasm` blob plus a signed
//! `manifest-v<N>.json`, with an `active` pointer naming the version in use. The
//! registry also keeps `runs.jsonl` — the recorded inputs and accepted outputs a
//! skill has seen — which is what lets the learning loop replay a candidate version
//! against real history instead of guessing. Every install and activation is
//! recorded in the audit ledger.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use engram_core::{now_ms, Ledger};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capability::Capability;
use crate::host::SkillError;
use crate::manifest::{module_hash, Manifest, SignedSkill, SkillSigner};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ledger: {0}")]
    Ledger(#[from] engram_core::LedgerError),
    #[error("manifest: {0}")]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error("skill: {0}")]
    Skill(#[from] SkillError),
    #[error("hex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("not found: {0}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, RegistryError>;

/// The fields needed to install a new skill version (the registry fills in version
/// and module hash, then signs).
#[derive(Debug, Clone)]
pub struct NewSkill {
    pub id: String,
    pub category: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub metric: String,
}

/// One recorded execution kept for replay: what went in, what output was accepted,
/// and how good it was (1.0 accept, 0.5 tweak, 0.0 reject).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedRun {
    pub version: u32,
    pub input_hex: String,
    pub gold_hex: String,
    pub reward: f32,
    pub ts_ms: u64,
}

pub struct Registry {
    dir: PathBuf,
    signer: Arc<SkillSigner>,
    ledger: Arc<Ledger>,
}

impl Registry {
    pub fn open(dir: impl AsRef<Path>, signer: Arc<SkillSigner>, ledger: Arc<Ledger>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(dir.join("skills"))?;
        Ok(Registry { dir, signer, ledger })
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn verifying(&self) -> &ed25519_dalek::VerifyingKey {
        &self.signer.verifying
    }

    fn skill_dir(&self, id: &str) -> PathBuf {
        self.dir.join("skills").join(id)
    }

    /// Install a new version: sign it, persist the bytes and manifest, and make it
    /// active if the skill has no active version yet. Returns the new version number.
    pub fn install(&self, new: NewSkill, wasm: &[u8]) -> Result<u32> {
        let sd = self.skill_dir(&new.id);
        fs::create_dir_all(&sd)?;
        let version = self.next_version(&new.id)?;
        let manifest = Manifest {
            id: new.id.clone(),
            version,
            category: new.category,
            description: new.description,
            entry: "run".into(),
            capabilities: new.capabilities,
            metric: new.metric,
            module_hash: module_hash(wasm),
        };
        let signed = self.signer.sign(&manifest)?;
        fs::write(sd.join(format!("v{version}.wasm")), wasm)?;
        fs::write(
            sd.join(format!("manifest-v{version}.json")),
            serde_json::to_vec_pretty(&signed)?,
        )?;
        if self.active_version(&new.id)?.is_none() {
            fs::write(sd.join("active"), version.to_string())?;
        }
        self.ledger.append(
            "skill.install",
            "core",
            json!({ "id": new.id, "version": version, "module_hash": manifest.module_hash }),
        )?;
        Ok(version)
    }

    /// Load a specific signed version and its bytes.
    pub fn load(&self, id: &str, version: u32) -> Result<(SignedSkill, Vec<u8>)> {
        let sd = self.skill_dir(id);
        let manifest_path = sd.join(format!("manifest-v{version}.json"));
        let wasm_path = sd.join(format!("v{version}.wasm"));
        if !manifest_path.exists() || !wasm_path.exists() {
            return Err(RegistryError::NotFound(format!("{id} v{version}")));
        }
        let signed: SignedSkill = serde_json::from_slice(&fs::read(manifest_path)?)?;
        let wasm = fs::read(wasm_path)?;
        Ok((signed, wasm))
    }

    /// Load the active version, if any.
    pub fn load_active(&self, id: &str) -> Result<(SignedSkill, Vec<u8>)> {
        let v = self
            .active_version(id)?
            .ok_or_else(|| RegistryError::NotFound(format!("{id} (no active version)")))?;
        self.load(id, v)
    }

    pub fn active_version(&self, id: &str) -> Result<Option<u32>> {
        let p = self.skill_dir(id).join("active");
        match fs::read_to_string(&p) {
            Ok(s) => Ok(s.trim().parse().ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn versions(&self, id: &str) -> Result<Vec<u32>> {
        let sd = self.skill_dir(id);
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(&sd) {
            for entry in rd.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(v) = name.strip_prefix('v').and_then(|s| s.strip_suffix(".wasm")) {
                        if let Ok(n) = v.parse() {
                            out.push(n);
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    fn next_version(&self, id: &str) -> Result<u32> {
        Ok(self.versions(id)?.into_iter().max().unwrap_or(0) + 1)
    }

    /// Point `active` at `version` (used by promote and revert). Records the change.
    pub fn set_active(&self, id: &str, version: u32, actor: &str, kind: &str) -> Result<()> {
        if !self.skill_dir(id).join(format!("v{version}.wasm")).exists() {
            return Err(RegistryError::NotFound(format!("{id} v{version}")));
        }
        let prev = self.active_version(id)?;
        fs::write(self.skill_dir(id).join("active"), version.to_string())?;
        self.ledger.append(
            kind,
            actor,
            json!({ "id": id, "from": prev, "to": version }),
        )?;
        Ok(())
    }

    /// Record an execution for later replay.
    pub fn record_run(&self, id: &str, version: u32, input: &[u8], gold: &[u8], reward: f32) -> Result<()> {
        use std::io::Write;
        let run = RecordedRun {
            version,
            input_hex: hex::encode(input),
            gold_hex: hex::encode(gold),
            reward,
            ts_ms: now_ms(),
        };
        let sd = self.skill_dir(id);
        fs::create_dir_all(&sd)?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(sd.join("runs.jsonl"))?;
        let mut line = serde_json::to_vec(&run)?;
        line.push(b'\n');
        f.write_all(&line)?;
        Ok(())
    }

    /// Accepted runs (reward ≥ 0.75) as (input, gold) pairs — the replay set.
    pub fn accepted_runs(&self, id: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let p = self.skill_dir(id).join("runs.jsonl");
        let content = match fs::read_to_string(&p) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let run: RecordedRun = serde_json::from_str(line)?;
            if run.reward >= 0.75 {
                out.push((hex::decode(&run.input_hex)?, hex::decode(&run.gold_hex)?));
            }
        }
        Ok(out)
    }
}
