//! The skill registry - procedural memory on disk.
//!
//! Skills live under `skills/<id>/`: each version is a `v<N>.wasm` blob plus a signed
//! `manifest-v<N>.json`, with an `active` pointer naming the version in use. The
//! registry also keeps `runs.jsonl` - the recorded inputs and accepted outputs a
//! skill has seen - which is what lets the learning loop replay a candidate version
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
use crate::manifest::{artifact_ext, module_hash, Manifest, Runtime, SignedSkill, SkillSigner};

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
    /// Substrate for this version. Defaults to `Wasm` (so existing call sites are unchanged); set to
    /// `Process` for a polyglot script skill.
    pub runtime: Runtime,
    /// Interpreter for a `Process` skill (e.g. "python3"). Ignored for WASM.
    pub interpreter: Option<String>,
    /// Optional natural-language selection cue.
    pub when_to_use: Option<String>,
}

impl NewSkill {
    /// A WASM skill with no extra metadata — the back-compat constructor for existing call sites.
    pub fn wasm(
        id: impl Into<String>,
        category: impl Into<String>,
        description: impl Into<String>,
        capabilities: Vec<Capability>,
        metric: impl Into<String>,
    ) -> Self {
        NewSkill {
            id: id.into(),
            category: category.into(),
            description: description.into(),
            capabilities,
            metric: metric.into(),
            runtime: Runtime::Wasm,
            interpreter: None,
            when_to_use: None,
        }
    }
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
    pub fn open(
        dir: impl AsRef<Path>,
        signer: Arc<SkillSigner>,
        ledger: Arc<Ledger>,
    ) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(dir.join("skills"))?;
        Ok(Registry {
            dir,
            signer,
            ledger,
        })
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn verifying(&self) -> &ed25519_dalek::VerifyingKey {
        &self.signer.verifying
    }

    /// Sign an autonomy policy with the registry's key — used when a human authors a durable agent's
    /// standing egress grant, so it verifies under [`verifying`](Self::verifying) at run construction.
    pub fn sign_autonomy(
        &self,
        policy: &engram_core::AutonomyPolicy,
    ) -> engram_core::SignedAutonomyPolicy {
        self.signer.sign_policy(policy)
    }

    fn skill_dir(&self, id: &str) -> PathBuf {
        self.dir.join("skills").join(id)
    }

    /// Locate the artifact file for a version. Skills are stored as `v{N}.<ext>` where the extension
    /// varies by runtime (wasm/py/js/sh/...), so we match on the `v{N}.` prefix rather than a fixed
    /// `.wasm` suffix. This is the single place the "any extension" rule is implemented.
    fn artifact_path(&self, id: &str, version: u32) -> Option<PathBuf> {
        let sd = self.skill_dir(id);
        let prefix = format!("v{version}.");
        let rd = fs::read_dir(&sd).ok()?;
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                // Exclude the manifest sidecar (manifest-v{N}.json) which does not start with "v{N}.".
                if name.starts_with(&prefix) {
                    return Some(entry.path());
                }
            }
        }
        None
    }

    /// Install a new version: sign it, persist the bytes and manifest, and make it
    /// active if the skill has no active version yet. Returns the new version number.
    pub fn install(&self, new: NewSkill, wasm: &[u8]) -> Result<u32> {
        self.install_inner(new, wasm, true)
    }

    /// Install a version WITHOUT activating it, even when the skill has no active version yet. Used
    /// by autonomous distillation: a proposed skill is parked inactive (invisible to skill_search /
    /// auto-select) until it is taught + improved or explicitly activated, so junk can't auto-run.
    pub fn install_inactive(&self, new: NewSkill, wasm: &[u8]) -> Result<u32> {
        self.install_inner(new, wasm, false)
    }

    fn install_inner(&self, new: NewSkill, wasm: &[u8], activate: bool) -> Result<u32> {
        let sd = self.skill_dir(&new.id);
        fs::create_dir_all(&sd)?;
        let version = self.next_version(&new.id)?;
        // A process skill's entry is its script filename; a WASM skill keeps the "run" export.
        let ext = artifact_ext(new.runtime, new.interpreter.as_deref());
        let entry = match new.runtime {
            Runtime::Wasm => "run".to_string(),
            Runtime::Process => format!("v{version}.{ext}"),
        };
        let manifest = Manifest {
            id: new.id.clone(),
            version,
            category: new.category,
            description: new.description,
            entry,
            capabilities: new.capabilities,
            metric: new.metric,
            module_hash: module_hash(wasm),
            runtime: new.runtime,
            interpreter: new.interpreter,
            when_to_use: new.when_to_use,
        };
        let signed = self.signer.sign(&manifest)?;
        fs::write(sd.join(format!("v{version}.{ext}")), wasm)?;
        fs::write(
            sd.join(format!("manifest-v{version}.json")),
            serde_json::to_vec_pretty(&signed)?,
        )?;
        if activate && self.active_version(&new.id)?.is_none() {
            fs::write(sd.join("active"), version.to_string())?;
        }
        self.ledger.append(
            "skill.install",
            "core",
            json!({ "id": new.id, "version": version, "module_hash": manifest.module_hash, "active": activate }),
        )?;
        Ok(version)
    }

    /// Soft-retire a skill: drop its active pointer (so it's hidden from listing/selection) and mark
    /// it retired, keeping the signed bytes + manifests on disk so it's recoverable (re-activate a
    /// version to bring it back). The retirement is signed into the ledger. Used by the skill-sleep
    /// prune to clear away proposed-but-never-adopted skills.
    pub fn retire(&self, id: &str, actor: &str) -> Result<()> {
        let sd = self.skill_dir(id);
        if !sd.exists() {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        let _ = fs::remove_file(sd.join("active"));
        fs::write(sd.join("retired"), b"1")?;
        self.ledger
            .append("skill.retire", actor, json!({ "id": id }))?;
        Ok(())
    }

    /// True if the skill has been soft-retired.
    pub fn is_retired(&self, id: &str) -> bool {
        self.skill_dir(id).join("retired").exists()
    }

    /// The user-facing on/off switch. Toggles ONLY the `retired` marker (the active pointer and all
    /// versions stay), so a disabled skill is hidden from selection/auto-use but instantly
    /// re-enablable and still inspectable in the UI — unlike [`retire`](Self::retire), which also
    /// drops the active pointer (used by the prune).
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let sd = self.skill_dir(id);
        if !sd.exists() {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        let marker = sd.join("retired");
        if enabled {
            let _ = fs::remove_file(&marker);
        } else {
            fs::write(&marker, b"1")?;
        }
        self.ledger
            .append("skill.toggle", "user", json!({ "id": id, "enabled": enabled }))?;
        Ok(())
    }

    /// List ALL skills including disabled ones — for the UI, which shows a disabled skill greyed with
    /// an on/off toggle. [`skills`](Self::skills) excludes disabled ones (selection/auto-use).
    pub fn skills_all(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(self.dir.join("skills")) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    if let Some(n) = e.file_name().to_str() {
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Load a specific signed version and its bytes.
    pub fn load(&self, id: &str, version: u32) -> Result<(SignedSkill, Vec<u8>)> {
        let sd = self.skill_dir(id);
        let manifest_path = sd.join(format!("manifest-v{version}.json"));
        let Some(artifact_path) = self.artifact_path(id, version) else {
            return Err(RegistryError::NotFound(format!("{id} v{version}")));
        };
        if !manifest_path.exists() {
            return Err(RegistryError::NotFound(format!("{id} v{version}")));
        }
        let signed: SignedSkill = serde_json::from_slice(&fs::read(manifest_path)?)?;
        let wasm = fs::read(artifact_path)?;
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
                    // Match an artifact `v{N}.<ext>` for ANY extension (wasm/py/js/sh/...). The
                    // manifest sidecar `manifest-v{N}.json` and the `active`/`runs.jsonl` files do
                    // not start with `v<digit>`, so they're skipped.
                    if let Some(rest) = name.strip_prefix('v') {
                        if let Some(num) = rest.split('.').next() {
                            if let Ok(n) = num.parse() {
                                if !out.contains(&n) {
                                    out.push(n);
                                }
                            }
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

    /// All installed skill ids.
    pub fn skills(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(self.dir.join("skills")) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    if let Some(n) = e.file_name().to_str() {
                        // Soft-retired skills are hidden from listing (and thus from selection/UI).
                        if e.path().join("retired").exists() {
                            continue;
                        }
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Point `active` at `version` (used by promote and revert). Records the change.
    pub fn set_active(&self, id: &str, version: u32, actor: &str, kind: &str) -> Result<()> {
        if self.artifact_path(id, version).is_none() {
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
    pub fn record_run(
        &self,
        id: &str,
        version: u32,
        input: &[u8],
        gold: &[u8],
        reward: f32,
    ) -> Result<()> {
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

    /// Accepted runs (reward ≥ 0.75) as (input, gold) pairs - the replay set.
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
