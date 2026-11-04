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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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
    #[error("invalid skill id: {0:?}")]
    InvalidId(String),
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
    /// Serialises version allocation and `active` pointer writes so concurrent installs
    /// of the same skill can't both claim the same version.
    write_lock: Mutex<()>,
}

/// A skill id maps to one directory name under `skills/`, so it must not be able to
/// name anything else (parent dirs, hidden files, absolute paths).
fn id_is_safe(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('.') && !id.contains(['/', '\\', '\0']) && !id.contains("..")
}

/// Replace `path` with `data` atomically: write a temp file in the same directory,
/// fsync it, then rename over the target. A crash leaves either the old or new contents.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let res = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
        return res;
    }
    // Persist the rename itself; not all platforms allow opening a dir, so best effort.
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Create `path` with `data`, failing if it already exists.
fn write_new(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()
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
            write_lock: Mutex::new(()),
        })
    }

    fn lock_writes(&self) -> MutexGuard<'_, ()> {
        // The guarded data is `()`, so a poisoned lock carries no broken state.
        self.write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    fn skill_dir(&self, id: &str) -> Result<PathBuf> {
        if !id_is_safe(id) {
            return Err(RegistryError::InvalidId(id.to_string()));
        }
        Ok(self.dir.join("skills").join(id))
    }

    /// Locate the artifact file for a version. Skills are stored as `v{N}.<ext>` where the extension
    /// varies by runtime (wasm/py/js/sh/...), so we match on the `v{N}.` prefix rather than a fixed
    /// `.wasm` suffix. This is the single place the "any extension" rule is implemented.
    fn artifact_path(&self, id: &str, version: u32) -> Option<PathBuf> {
        let sd = self.skill_dir(id).ok()?;
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
        let sd = self.skill_dir(&new.id)?;
        // Held from version allocation through the ledger append.
        let _guard = self.lock_writes();
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
        // create_new: if another process already claimed this version, fail instead of
        // overwriting it. The manifest name doesn't depend on the extension, so it is the
        // reservation that catches a racing install of a different runtime.
        let artifact = sd.join(format!("v{version}.{ext}"));
        write_new(&artifact, wasm)?;
        if let Err(e) = write_new(
            &sd.join(format!("manifest-v{version}.json")),
            &serde_json::to_vec_pretty(&signed)?,
        ) {
            let _ = fs::remove_file(&artifact);
            return Err(e.into());
        }
        if activate && self.active_version(&new.id)?.is_none() {
            write_atomic(&sd.join("active"), version.to_string().as_bytes())?;
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
        let sd = self
            .skill_dir(id)
            .map_err(|_| RegistryError::NotFound(id.to_string()))?;
        if !sd.exists() {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        let _guard = self.lock_writes();
        let _ = fs::remove_file(sd.join("active"));
        fs::write(sd.join("retired"), b"1")?;
        self.ledger
            .append("skill.retire", actor, json!({ "id": id }))?;
        Ok(())
    }

    /// True if the skill has been soft-retired.
    pub fn is_retired(&self, id: &str) -> bool {
        self.skill_dir(id)
            .map(|sd| sd.join("retired").exists())
            .unwrap_or(false)
    }

    /// The user-facing on/off switch. Toggles ONLY the `retired` marker (the active pointer and all
    /// versions stay), so a disabled skill is hidden from selection/auto-use but instantly
    /// re-enablable and still inspectable in the UI — unlike [`retire`](Self::retire), which also
    /// drops the active pointer (used by the prune).
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let sd = self
            .skill_dir(id)
            .map_err(|_| RegistryError::NotFound(id.to_string()))?;
        if !sd.exists() {
            return Err(RegistryError::NotFound(id.to_string()));
        }
        let marker = sd.join("retired");
        if enabled {
            let _ = fs::remove_file(&marker);
        } else {
            fs::write(&marker, b"1")?;
        }
        self.ledger.append(
            "skill.toggle",
            "user",
            json!({ "id": id, "enabled": enabled }),
        )?;
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
        let sd = self.skill_dir(id)?;
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
        // An unsafe id can't name a skill, so it has no active version.
        let Ok(sd) = self.skill_dir(id) else {
            return Ok(None);
        };
        let p = sd.join("active");
        match fs::read_to_string(&p) {
            Ok(s) => Ok(s.trim().parse().ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn versions(&self, id: &str) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let Ok(sd) = self.skill_dir(id) else {
            return Ok(out);
        };
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
        let sd = self.skill_dir(id)?;
        let _guard = self.lock_writes();
        let prev = self.active_version(id)?;
        write_atomic(&sd.join("active"), version.to_string().as_bytes())?;
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
        let sd = self.skill_dir(id)?;
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
        let Ok(sd) = self.skill_dir(id) else {
            return Ok(Vec::new());
        };
        let p = sd.join("runs.jsonl");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Arc<Registry>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let signer = Arc::new(SkillSigner::load_or_create(dir.path().join("skill.key")).unwrap());
        let reg = Arc::new(Registry::open(dir.path(), signer, ledger).unwrap());
        (reg, dir)
    }

    fn skill(id: &str) -> NewSkill {
        NewSkill::wasm(id, "test", "a test skill", vec![], "exact_match")
    }

    #[test]
    fn concurrent_installs_get_distinct_versions() {
        let (reg, _dir) = setup();
        let n = 16;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let reg = reg.clone();
                std::thread::spawn(move || {
                    let bytes = format!("module-{i}").into_bytes();
                    let v = reg.install(skill("racer"), &bytes).unwrap();
                    (v, bytes)
                })
            })
            .collect();
        let results: Vec<(u32, Vec<u8>)> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let mut versions: Vec<u32> = results.iter().map(|(v, _)| *v).collect();
        versions.sort_unstable();
        assert_eq!(versions, (1..=n).collect::<Vec<u32>>());
        assert_eq!(reg.versions("racer").unwrap(), versions);

        // Each version's bytes and manifest belong to the install that claimed it.
        for (v, bytes) in &results {
            let (signed, loaded) = reg.load("racer", *v).unwrap();
            assert_eq!(&loaded, bytes);
            assert_eq!(signed.manifest.version, *v);
            assert_eq!(signed.manifest.module_hash, module_hash(bytes));
            crate::manifest::verify(&signed, &loaded, reg.verifying()).unwrap();
        }

        // One ledger install entry per version, no duplicates.
        let mut logged: Vec<u64> = reg
            .ledger()
            .read_all()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "skill.install")
            .map(|e| {
                let v: serde_json::Value = serde_json::from_str(e.payload.get()).unwrap();
                v["version"].as_u64().unwrap()
            })
            .collect();
        logged.sort_unstable();
        assert_eq!(logged, (1..=n as u64).collect::<Vec<u64>>());
        assert_eq!(reg.active_version("racer").unwrap(), Some(1));
    }

    #[test]
    fn install_refuses_to_overwrite_an_existing_version() {
        let (reg, dir) = setup();
        assert_eq!(reg.install(skill("s"), b"one").unwrap(), 1);
        // Simulate another process having claimed v2 with a different runtime's artifact.
        let sd = dir.path().join("skills").join("s");
        fs::write(sd.join("manifest-v2.json"), b"{}").unwrap();
        assert!(reg.install(skill("s"), b"two").is_err());
        assert!(!sd.join("v2.wasm").exists());
    }

    #[test]
    fn unsafe_ids_are_rejected() {
        let (reg, dir) = setup();
        for id in [
            "../x",
            "..",
            ".",
            ".hidden",
            "a/b",
            "a\\b",
            "",
            "a\0b",
            "x/../../y",
        ] {
            assert!(
                matches!(
                    reg.install(skill(id), b"m"),
                    Err(RegistryError::InvalidId(_))
                ),
                "{id:?}"
            );
            assert!(reg.record_run(id, 1, b"i", b"o", 1.0).is_err());
            assert!(reg.set_active(id, 1, "user", "skill.promote").is_err());
            assert!(reg.retire(id, "user").is_err());
            assert!(reg.set_enabled(id, false).is_err());
            assert!(reg.load(id, 1).is_err());
            assert_eq!(reg.active_version(id).unwrap(), None);
            assert!(reg.versions(id).unwrap().is_empty());
            assert!(reg.accepted_runs(id).unwrap().is_empty());
            assert!(!reg.is_retired(id));
        }
        // Nothing was created next to the skills dir.
        assert!(!dir.path().join("x").exists());
        assert!(!dir.path().join("y").exists());
        assert!(reg.skills_all().unwrap().is_empty());
    }

    #[test]
    fn active_pointer_is_replaced_atomically() {
        let (reg, dir) = setup();
        reg.install(skill("s"), b"one").unwrap();
        reg.install(skill("s"), b"two").unwrap();
        reg.set_active("s", 2, "user", "skill.promote").unwrap();
        assert_eq!(reg.active_version("s").unwrap(), Some(2));
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("skills").join("s"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn ordinary_ids_still_work() {
        let (reg, _dir) = setup();
        for id in ["csv2json", "web_fetch-v2", "a.b"] {
            assert_eq!(reg.install(skill(id), b"m").unwrap(), 1, "{id}");
            assert_eq!(reg.active_version(id).unwrap(), Some(1));
        }
    }
}
