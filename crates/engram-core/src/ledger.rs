//! The audit ledger - Engram's incorruptible memory of what it did.
//!
//! Every state change worth trusting (a memory write, a skill mutation, a revert)
//! is appended here *before or as* it happens. The ledger is:
//!
//! - **Append-only**: entries are never edited or deleted, only added.
//! - **Content-addressed**: each entry's id is the BLAKE3 hash of its contents.
//! - **Hash-chained**: each entry commits to the previous entry's hash, so altering
//!   any past entry breaks every hash after it - tampering is detectable in O(n).
//! - **Signed**: each entry is signed with the core's Ed25519 key, which lives on
//!   disk at `0600` and is *never* handed to a skill/WASM. A forged entry cannot be
//!   signed.
//!
//! This is what makes "transparent and auditable" real: the desktop (or anyone with
//! the public key) can replay the chain and prove nothing was rewritten. A "revert"
//! is itself an appended entry pointing at a prior good hash - history is added to,
//! never erased.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::event::now_ms;

/// 32-byte BLAKE3 digest.
pub type Hash = [u8; 32];
/// The chain's root: an all-zero hash that the first entry commits to.
pub const GENESIS: Hash = [0u8; 32];

const DOMAIN: &[u8] = b"engram.ledger.v1";

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("randomness unavailable: {0}")]
    Rand(String),
    #[error("bad key material")]
    Key,
    #[error("chain broken at seq {seq}: {reason}")]
    Broken { seq: u64, reason: String },
}

/// One immutable record in the chain. Hashes and signature are stored hex-encoded so
/// the on-disk JSONL is human-readable and independently verifiable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// 1-based position in the chain.
    pub seq: u64,
    /// Unix epoch milliseconds.
    pub ts_ms: u64,
    /// Hex of the previous entry's hash (GENESIS for the first entry).
    pub prev: String,
    /// What kind of change this is, e.g. `"memory.write"`, `"skill.promote"`, `"revert"`.
    pub kind: String,
    /// Who caused it, e.g. `"core"`, `"skill:drafter@3"`, `"user"`.
    pub actor: String,
    /// The change itself, kept as the exact JSON text that was written. Storing the
    /// raw bytes (rather than a re-parsed value) is what makes the content hash stable:
    /// the verifier hashes the identical bytes, immune to float-formatting round-trips.
    pub payload: Box<RawValue>,
    /// Hex of this entry's content hash (its content-address).
    pub hash: String,
    /// Hex of the Ed25519 signature over the content hash.
    pub sig: String,
}

/// Domain-separated, length-prefixed content hash. Length prefixes make the field
/// boundaries unambiguous, so no two distinct entries can collide by concatenation.
fn content_hash(seq: u64, ts_ms: u64, prev: &Hash, kind: &str, actor: &str, payload_raw: &str) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(DOMAIN);
    h.update(&seq.to_le_bytes());
    h.update(&ts_ms.to_le_bytes());
    h.update(prev);
    write_field(&mut h, kind.as_bytes());
    write_field(&mut h, actor.as_bytes());
    // Hash the exact payload bytes that get persisted - never a re-serialization.
    write_field(&mut h, payload_raw.as_bytes());
    *h.finalize().as_bytes()
}

fn write_field(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn to_hash(hex_str: &str) -> Result<Hash, LedgerError> {
    let v = hex::decode(hex_str)?;
    v.try_into().map_err(|_| LedgerError::Key)
}

struct Tip {
    file: File,
    seq: u64,
    prev: Hash,
}

/// The append-only ledger. Cheap to share behind an `Arc`; appends are serialized.
pub struct Ledger {
    path: PathBuf,
    tip: Mutex<Tip>,
    signing: SigningKey,
    /// Public key - hand this to the desktop/auditor to verify the chain.
    pub verifying: VerifyingKey,
}

impl Ledger {
    /// Open (or create) the ledger under `dir`. Loads the signing key from
    /// `dir/keys/ledger.key`, generating one on first run, and replays the existing
    /// `dir/ledger.jsonl` to find the chain tip.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let signing = load_or_create_key(&dir.join("keys").join("ledger.key"))?;
        let verifying = signing.verifying_key();

        let path = dir.join("ledger.jsonl");
        let (seq, prev) = replay_tip(&path)?;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        Ok(Ledger {
            path,
            tip: Mutex::new(Tip { file, seq, prev }),
            signing,
            verifying,
        })
    }

    /// Append a new record and return it. Serializes concurrent callers.
    pub fn append(
        &self,
        kind: impl Into<String>,
        actor: impl Into<String>,
        payload: serde_json::Value,
    ) -> Result<Entry, LedgerError> {
        let kind = kind.into();
        let actor = actor.into();
        let mut tip = self.tip.lock().expect("ledger mutex poisoned");

        let seq = tip.seq + 1;
        let ts_ms = now_ms();
        let payload = serde_json::value::to_raw_value(&payload)?;
        let hash = content_hash(seq, ts_ms, &tip.prev, &kind, &actor, payload.get());
        let sig = self.signing.sign(&hash);

        let entry = Entry {
            seq,
            ts_ms,
            prev: hex::encode(tip.prev),
            kind,
            actor,
            payload,
            hash: hex::encode(hash),
            sig: hex::encode(sig.to_bytes()),
        };

        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        tip.file.write_all(&line)?;
        tip.file.flush()?;

        tip.seq = seq;
        tip.prev = hash;
        Ok(entry)
    }

    /// Record a revert pointing at a prior known-good `target` hash. History is added
    /// to, not erased; the owning store interprets the revert and rolls its state back.
    pub fn revert(
        &self,
        actor: impl Into<String>,
        target_hash: &str,
        reason: impl Into<String>,
    ) -> Result<Entry, LedgerError> {
        self.append(
            "revert",
            actor,
            serde_json::json!({ "target": target_hash, "reason": reason.into() }),
        )
    }

    /// Current chain tip: (seq, hex hash). seq 0 / GENESIS means empty.
    pub fn head(&self) -> (u64, String) {
        let tip = self.tip.lock().expect("ledger mutex poisoned");
        (tip.seq, hex::encode(tip.prev))
    }

    /// Replay the whole chain from disk, checking every hash, signature, and link.
    /// Returns the number of valid entries or the first break found.
    pub fn verify(&self) -> Result<u64, LedgerError> {
        verify_file(&self.path, &self.verifying)
    }

    /// The ledger's public key, hex-encoded. Publish this once (out of band) so a third
    /// party can verify any future ledger offline without trusting the running daemon.
    pub fn pubkey_hex(&self) -> String {
        hex::encode(self.verifying.to_bytes())
    }

    /// Read every entry from disk (for the audit UI and tooling). Does not verify;
    /// pair with [`verify`](Self::verify) when integrity matters.
    pub fn read_all(&self) -> Result<Vec<Entry>, LedgerError> {
        read_entries(&self.path)
    }

    /// Read the last `n` entries - the live tail for the "Live Cortex" feed.
    pub fn tail(&self, n: usize) -> Result<Vec<Entry>, LedgerError> {
        let mut all = read_entries(&self.path)?;
        let start = all.len().saturating_sub(n);
        Ok(all.split_off(start))
    }
}

fn read_entries(path: &Path) -> Result<Vec<Entry>, LedgerError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let raw: Vec<String> = BufReader::new(file).lines().collect::<Result<_, _>>()?;
    let nonempty: Vec<&str> = raw.iter().map(String::as_str).filter(|l| !l.trim().is_empty()).collect();
    let mut out = Vec::with_capacity(nonempty.len());
    for (i, line) in nonempty.iter().enumerate() {
        match serde_json::from_str::<Entry>(line) {
            Ok(e) => out.push(e),
            // Tolerate a single unparseable *trailing* line - an append interrupted by a
            // crash/partial write. A parse failure on any earlier line is real corruption.
            Err(_) if i + 1 == nonempty.len() => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Build a public key from its hex encoding - for offline, third-party verification.
pub fn verifying_key_from_hex(s: &str) -> Result<VerifyingKey, LedgerError> {
    let bytes = hex::decode(s.trim()).map_err(|_| LedgerError::Key)?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| LedgerError::Key)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| LedgerError::Key)
}

/// Verify a ledger file against a public key without holding a `Ledger`.
pub fn verify_file(path: &Path, verifying: &VerifyingKey) -> Result<u64, LedgerError> {
    let entries = read_entries(path)?;
    let mut prev = GENESIS;
    let mut expect_seq = 0u64;
    for entry in &entries {
        expect_seq += 1;
        if entry.seq != expect_seq {
            return Err(LedgerError::Broken {
                seq: entry.seq,
                reason: format!("expected seq {expect_seq}"),
            });
        }
        if to_hash(&entry.prev)? != prev {
            return Err(LedgerError::Broken {
                seq: entry.seq,
                reason: "prev hash does not match running tip".into(),
            });
        }
        let recomputed = content_hash(
            entry.seq,
            entry.ts_ms,
            &prev,
            &entry.kind,
            &entry.actor,
            entry.payload.get(),
        );
        let stored = to_hash(&entry.hash)?;
        if recomputed != stored {
            return Err(LedgerError::Broken {
                seq: entry.seq,
                reason: "content hash mismatch (entry was altered)".into(),
            });
        }
        let sig_bytes: [u8; 64] = hex::decode(&entry.sig)?
            .try_into()
            .map_err(|_| LedgerError::Key)?;
        let sig = Signature::from_bytes(&sig_bytes);
        if verifying.verify(&stored, &sig).is_err() {
            return Err(LedgerError::Broken {
                seq: entry.seq,
                reason: "signature invalid".into(),
            });
        }
        prev = stored;
    }
    Ok(expect_seq)
}

fn replay_tip(path: &Path) -> Result<(u64, Hash), LedgerError> {
    match read_entries(path)?.last() {
        Some(entry) => Ok((entry.seq, to_hash(&entry.hash)?)),
        None => Ok((0, GENESIS)),
    }
}

fn load_or_create_key(path: &Path) -> Result<SigningKey, LedgerError> {
    if let Ok(bytes) = std::fs::read(path) {
        let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| LedgerError::Key)?;
        return Ok(SigningKey::from_bytes(&seed));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| LedgerError::Rand(e.to_string()))?;
    write_secret(path, &seed)?;
    Ok(SigningKey::from_bytes(&seed))
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), LedgerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = OpenOptions::new()
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
    use serde_json::json;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn appends_and_verifies() {
        let dir = temp();
        let l = Ledger::open(dir.path()).unwrap();
        let e1 = l.append("memory.write", "core", json!({ "fact": "user likes Rust" })).unwrap();
        let e2 = l.append("skill.promote", "user", json!({ "skill": "drafter", "v": 3 })).unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);
        assert_eq!(e2.prev, e1.hash); // chain links
        assert_eq!(l.verify().unwrap(), 2);
        assert_eq!(l.head().0, 2);
    }

    #[test]
    fn persists_across_reopen() {
        let dir = temp();
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("a", "core", json!(1)).unwrap();
            l.append("b", "core", json!(2)).unwrap();
        }
        let l = Ledger::open(dir.path()).unwrap();
        let e3 = l.append("c", "core", json!(3)).unwrap();
        assert_eq!(e3.seq, 3); // continued, not reset
        assert_eq!(l.verify().unwrap(), 3);
    }

    #[test]
    fn detects_tampering() {
        let dir = temp();
        let path = dir.path().join("ledger.jsonl");
        let verifying;
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("memory.write", "core", json!({ "amount": 10 })).unwrap();
            l.append("memory.write", "core", json!({ "amount": 20 })).unwrap();
            verifying = l.verifying;
        }
        // Rewrite the first entry's payload on disk, keeping its old hash/sig.
        let mut lines: Vec<String> =
            std::fs::read_to_string(&path).unwrap().lines().map(String::from).collect();
        lines[0] = lines[0].replace("\"amount\":10", "\"amount\":9999");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let err = verify_file(&path, &verifying).unwrap_err();
        match err {
            LedgerError::Broken { seq, .. } => assert_eq!(seq, 1),
            other => panic!("expected Broken, got {other:?}"),
        }
    }

    #[test]
    fn tolerates_a_partial_trailing_line() {
        let dir = temp();
        let path = dir.path().join("ledger.jsonl");
        let verifying;
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("a", "core", json!(1)).unwrap();
            l.append("b", "core", json!(2)).unwrap();
            verifying = l.verifying;
        }
        // Simulate a crash mid-append: a truncated JSON fragment with no trailing newline.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"seq\":3,\"kind\":\"c\",\"actor\":\"core\",\"payl").unwrap();
        drop(f);
        // The ledger still loads (not bricked) and verifies its complete prefix.
        let l = Ledger::open(dir.path()).unwrap();
        assert_eq!(l.head().0, 2);
        assert_eq!(verify_file(&path, &verifying).unwrap(), 2);
    }

    #[test]
    fn exported_pubkey_verifies_the_chain_offline() {
        let dir = temp();
        let l = Ledger::open(dir.path()).unwrap();
        l.append("a", "core", json!(1)).unwrap();
        l.append("b", "core", json!(2)).unwrap();
        // The published public key (hex) reconstructs and verifies the signed file.
        let vk = verifying_key_from_hex(&l.pubkey_hex()).unwrap();
        assert_eq!(verify_file(&dir.path().join("ledger.jsonl"), &vk).unwrap(), 2);
        // Bad hex is an error, never a panic.
        assert!(verifying_key_from_hex("not-hex").is_err());
    }

    #[test]
    fn revert_is_appended_not_erased() {
        let dir = temp();
        let l = Ledger::open(dir.path()).unwrap();
        let bad = l.append("skill.mutate", "skill:drafter@4", json!({ "regression": true })).unwrap();
        let rev = l.revert("user", &bad.hash, "metric dropped").unwrap();
        assert_eq!(rev.kind, "revert");
        let payload: serde_json::Value = serde_json::from_str(rev.payload.get()).unwrap();
        assert_eq!(payload["target"], bad.hash);
        assert_eq!(l.verify().unwrap(), 2); // both entries remain, chain intact
    }
}
