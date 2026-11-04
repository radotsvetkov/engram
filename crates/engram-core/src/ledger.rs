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
//! - **Anchored**: a signed head checkpoint (`ledger.head`: the latest seq and hash) is kept
//!   next to the chain - refreshed at most once a second during appends (after flushing the
//!   chain to disk), and whenever the ledger is verified or closed. Verification requires the chain to reach
//!   it, so deleting entries from the END - which the hash chain alone can't see - is caught
//!   too. A head saved elsewhere ([`Ledger::signed_head`]) can be checked the same way with
//!   [`verify_file_with_anchor`], which also covers someone deleting the local head file.
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
/// Domain tag for head-checkpoint signatures, so a head signature can never be mistaken for an
/// entry signature (entries sign a 32-byte content hash; heads sign this tag + seq + hash).
const HEAD_DOMAIN: &[u8] = b"engram.ledger.head.v1";
/// File name of the head checkpoint, next to `ledger.jsonl`.
const HEAD_FILE: &str = "ledger.head";

/// A signed statement "the chain had reached `seq`, whose entry hash was `hash`". Written to
/// `ledger.head` after every append, and exportable so an auditor can keep one off the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHead {
    pub seq: u64,
    /// Hex of the entry hash at `seq`.
    pub hash: String,
    /// Hex Ed25519 signature over `HEAD_DOMAIN || seq (LE) || hash`.
    pub sig: String,
}

fn head_message(seq: u64, hash: &Hash) -> Vec<u8> {
    let mut m = Vec::with_capacity(HEAD_DOMAIN.len() + 8 + 32);
    m.extend_from_slice(HEAD_DOMAIN);
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(hash);
    m
}

impl SignedHead {
    fn sign(signing: &SigningKey, seq: u64, hash: &Hash) -> Self {
        SignedHead {
            seq,
            hash: hex::encode(hash),
            sig: hex::encode(signing.sign(&head_message(seq, hash)).to_bytes()),
        }
    }

    /// Check the signature. A head that fails this was not written by the keyholder.
    pub fn verify_sig(&self, verifying: &VerifyingKey) -> Result<(), LedgerError> {
        let malformed = || LedgerError::Broken {
            seq: self.seq,
            reason: "head checkpoint is malformed".into(),
        };
        let hash = to_hash(&self.hash).map_err(|_| malformed())?;
        let sig_bytes: [u8; 64] = hex::decode(&self.sig)
            .map_err(|_| malformed())?
            .try_into()
            .map_err(|_| malformed())?;
        verifying
            .verify(
                &head_message(self.seq, &hash),
                &Signature::from_bytes(&sig_bytes),
            )
            .map_err(|_| LedgerError::Broken {
                seq: self.seq,
                reason: "head checkpoint signature invalid".into(),
            })
    }
}

/// The local head, if it exists and the chain no longer reaches it (or it isn't validly
/// signed). `None` when there is no head or it matches.
fn rejected_head(ledger: &Path, entries: &[Entry], verifying: &VerifyingKey) -> Option<SignedHead> {
    let head = match read_head(ledger) {
        Ok(Some(h)) => h,
        Ok(None) => return None,
        // Unreadable: keep whatever is there as evidence too.
        Err(_) => {
            return Some(SignedHead {
                seq: 0,
                hash: String::new(),
                sig: String::new(),
            })
        }
    };
    let hashes: Vec<Hash> = entries
        .iter()
        .filter_map(|e| to_hash(&e.hash).ok())
        .collect();
    match check_reaches(&hashes, &head, verifying) {
        Ok(()) => None,
        Err(_) => Some(head),
    }
}

/// Heads that were set aside because the chain no longer reached them
/// (`ledger.head.rejected-<ms>`). Verification fails while any exist.
fn rejected_head_files(ledger: &Path) -> Vec<PathBuf> {
    let dir = ledger.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!("{HEAD_FILE}.rejected-");
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    out.sort();
    out
}

/// Path of the head checkpoint that belongs to a ledger file.
fn head_path(ledger: &Path) -> PathBuf {
    ledger.with_file_name(HEAD_FILE)
}

/// Write the head checkpoint atomically (temp file, fsync, rename) so a crash leaves either the
/// old head or the new one, never a torn file.
fn write_head(ledger: &Path, head: &SignedHead) -> Result<(), LedgerError> {
    let path = head_path(ledger);
    let tmp = path.with_file_name(format!(".{HEAD_FILE}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(head)?;
    let result = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_data()?;
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Read the head checkpoint next to `ledger`, if there is one.
fn read_head(ledger: &Path) -> Result<Option<SignedHead>, LedgerError> {
    match std::fs::read(head_path(ledger)) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|_| LedgerError::Broken {
                seq: 0,
                reason: "head checkpoint (ledger.head) is unreadable".into(),
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

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
fn content_hash(
    seq: u64,
    ts_ms: u64,
    prev: &Hash,
    kind: &str,
    actor: &str,
    payload_raw: &str,
) -> Hash {
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
    /// The seq the on-disk head checkpoint was last written for, and when.
    head_seq: u64,
    head_written_ms: u64,
}

/// How often the head checkpoint is refreshed during a burst of appends. Writing it means
/// flushing the ledger to disk first (so the head never points past a durable entry), which
/// is too slow to do on every append.
const HEAD_INTERVAL_MS: u64 = 1000;

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
        let entries = read_entries(&path)?;
        let (seq, prev) = match entries.last() {
            Some(e) => (e.seq, to_hash(&e.hash)?),
            None => (0, GENESIS),
        };
        // Drop the torn tail of an interrupted append so the next append starts on a clean
        // line; otherwise it would merge with the fragment and corrupt the chain mid-file.
        truncate_torn_tail(&path)?;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        let ledger = Ledger {
            path,
            tip: Mutex::new(Tip {
                file,
                seq,
                prev,
                head_seq: 0,
                head_written_ms: 0,
            }),
            signing,
            verifying,
        };
        // The existing head must still be reachable. If entries were cut off the end while we
        // weren't running, the head is ahead of (or disagrees with) the chain. Rewriting it from
        // the shortened chain would erase the evidence, so keep the old head beside the ledger
        // (verification keeps failing while it exists) and record the event in the chain.
        if let Some(rejected) = rejected_head(&ledger.path, &entries, &ledger.verifying) {
            let kept = head_path(&ledger.path)
                .with_file_name(format!("{HEAD_FILE}.rejected-{}", now_ms()));
            std::fs::rename(head_path(&ledger.path), &kept)?;
            tracing::error!(
                head_seq = rejected.seq,
                chain_seq = seq,
                kept = %kept.display(),
                "ledger head checkpoint does not match the chain: entries were removed or rewritten"
            );
            ledger.append(
                "ledger.truncation_detected",
                "core",
                serde_json::json!({
                    "head": rejected,
                    "chain_seq": seq,
                    "kept_as": kept.file_name().map(|n| n.to_string_lossy().into_owned()),
                }),
            )?;
            ledger.sync_head()?;
        }
        // A non-empty chain with no head checkpoint: the first run of a version that writes
        // one, or the file was removed. Record that in the chain itself (the append also writes
        // a fresh head), so a removed checkpoint leaves a visible, signed trace in the audit log.
        if seq > 0 && !head_path(&ledger.path).exists() {
            ledger.append(
                "ledger.head_created",
                "core",
                serde_json::json!({
                    "at_seq": seq,
                    "reason": "no head checkpoint was found (first run with head checkpoints, or ledger.head was removed)",
                }),
            )?;
            ledger.sync_head()?;
        }
        Ok(ledger)
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
        // On a failed write, cut the file back so no partial line is left behind.
        let len_before = tip.file.metadata()?.len();
        if let Err(e) = tip.file.write_all(&line).and_then(|_| tip.file.flush()) {
            let _ = tip.file.set_len(len_before);
            return Err(e.into());
        }

        tip.seq = seq;
        tip.prev = hash;
        // A failure here only means the head lags, which verification accepts (the chain may run
        // past the head, never stop short of it).
        if let Err(e) = self.refresh_head(&mut tip, false) {
            tracing::warn!(error = %e, "could not update the ledger head checkpoint");
        }
        Ok(entry)
    }

    /// Bring the on-disk head checkpoint up to the current tip now (rather than within
    /// `HEAD_INTERVAL_MS`). Called before verifying and when the ledger is dropped.
    pub fn sync_head(&self) -> Result<(), LedgerError> {
        let mut tip = self.tip.lock().expect("ledger mutex poisoned");
        self.refresh_head(&mut tip, true)
    }

    /// Write the head for the current tip if it is stale and (unless `force`) the interval has
    /// passed. Runs under the tip lock, so heads are written in chain order. The ledger file is
    /// flushed to disk first: a head must never survive a power cut that its entry didn't.
    fn refresh_head(&self, tip: &mut Tip, force: bool) -> Result<(), LedgerError> {
        if tip.seq == 0 || tip.seq == tip.head_seq {
            return Ok(());
        }
        let now = now_ms();
        if !force && now.saturating_sub(tip.head_written_ms) < HEAD_INTERVAL_MS {
            return Ok(());
        }
        tip.file.sync_data()?;
        write_head(
            &self.path,
            &SignedHead::sign(&self.signing, tip.seq, &tip.prev),
        )?;
        tip.head_seq = tip.seq;
        tip.head_written_ms = now;
        Ok(())
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

    /// The current tip as a signed checkpoint. Save it somewhere the host can't rewrite (another
    /// machine, a printout, a transparency log); [`verify_file_with_anchor`] later proves the
    /// chain still contains it and has only grown since.
    ///
    /// Flushes the chain (and the local head) to disk first, so an exported head never points
    /// past an entry a power cut could still lose.
    pub fn signed_head(&self) -> Result<SignedHead, LedgerError> {
        let mut tip = self.tip.lock().expect("ledger mutex poisoned");
        self.refresh_head(&mut tip, true)?;
        Ok(SignedHead::sign(&self.signing, tip.seq, &tip.prev))
    }

    /// Replay the whole chain from disk, checking every hash, signature, and link.
    /// Returns the number of valid entries or the first break found.
    pub fn verify(&self) -> Result<u64, LedgerError> {
        // Make the head current first, so entries appended moments ago are covered too.
        if let Err(e) = self.sync_head() {
            tracing::warn!(error = %e, "could not update the ledger head checkpoint");
        }
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
    ///
    /// Reads backwards from the end of the file only as far as it needs to, so polling the tail
    /// stays cheap however long the ledger grows (it used to parse the whole file each time).
    pub fn tail(&self, n: usize) -> Result<Vec<Entry>, LedgerError> {
        read_tail(&self.path, n)
    }
}

fn read_entries(path: &Path) -> Result<Vec<Entry>, LedgerError> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let raw: Vec<String> = BufReader::new(file).lines().collect::<Result<_, _>>()?;
    let nonempty: Vec<&str> = raw
        .iter()
        .map(String::as_str)
        .filter(|l| !l.trim().is_empty())
        .collect();
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

impl Drop for Ledger {
    fn drop(&mut self) {
        // Leave the head current on a clean shutdown. Never panic here (a poisoned lock just
        // means the head isn't refreshed).
        if let Ok(mut tip) = self.tip.lock() {
            let _ = self.refresh_head(&mut tip, true);
        }
    }
}

/// The last `n` entries of the file, reading backwards in chunks. Same result as taking the tail
/// of `read_entries`: blank lines are ignored and one unparseable trailing line (an interrupted
/// append) is skipped; any other unparseable line in the window is an error.
fn read_tail(path: &Path, n: usize) -> Result<Vec<Entry>, LedgerError> {
    use std::io::{Read, Seek, SeekFrom};
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    const CHUNK: u64 = 64 * 1024;
    let len = file.metadata()?.len();
    let mut pos = len;
    // Bytes from `pos` to the end of the file.
    let mut buf: Vec<u8> = Vec::new();
    // Read until the window holds n+1 complete lines (one extra for a torn trailing line and one
    // for the possibly partial first line), or the whole file.
    loop {
        let lines = buf.iter().filter(|b| **b == b'\n').count();
        if pos == 0 || lines > n + 1 {
            break;
        }
        let step = CHUNK.min(pos);
        pos -= step;
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&buf);
        buf = chunk;
    }
    // Unless we reached the start of the file, the first line in the window may be partial.
    let window: &[u8] = if pos == 0 {
        &buf
    } else {
        match buf.iter().position(|b| *b == b'\n') {
            Some(i) => &buf[i + 1..],
            None => &[],
        }
    };
    let text = String::from_utf8_lossy(window);
    let nonempty: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut out = Vec::with_capacity(nonempty.len());
    for (i, line) in nonempty.iter().enumerate() {
        match serde_json::from_str::<Entry>(line) {
            Ok(e) => out.push(e),
            Err(_) if i + 1 == nonempty.len() => break,
            Err(e) => return Err(e.into()),
        }
    }
    let start = out.len().saturating_sub(n);
    Ok(out.split_off(start))
}

/// Read every entry from a ledger file WITHOUT a signing key — for offline auditing/tooling that has
/// only the JSONL (and, separately, the public key for [`verify_file`]). Does not verify; pair with
/// [`verify_file`] when integrity matters.
pub fn entries_from_file(path: &Path) -> Result<Vec<Entry>, LedgerError> {
    read_entries(path)
}

/// Build a public key from its hex encoding - for offline, third-party verification.
pub fn verifying_key_from_hex(s: &str) -> Result<VerifyingKey, LedgerError> {
    let bytes = hex::decode(s.trim()).map_err(|_| LedgerError::Key)?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| LedgerError::Key)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| LedgerError::Key)
}

/// Verify a ledger file against a public key without holding a `Ledger`.
pub fn verify_file(path: &Path, verifying: &VerifyingKey) -> Result<u64, LedgerError> {
    verify_file_with_anchor(path, verifying, None)
}

/// [`verify_file`], plus a check that the chain reaches `anchor`: a signed head saved earlier,
/// possibly on another machine. The local `ledger.head` is always checked when present; the
/// anchor additionally catches truncation by someone who also removed or rolled back that file.
pub fn verify_file_with_anchor(
    path: &Path,
    verifying: &VerifyingKey,
    anchor: Option<&SignedHead>,
) -> Result<u64, LedgerError> {
    // Read the head BEFORE the chain: a head is only written after its entry, so the chain read
    // afterwards always reaches it, even while the daemon keeps appending. (In the other order an
    // append landing between the two reads would look like truncation.)
    let local = read_head(path)?;
    let hashes = verify_chain(path, verifying)?;
    for head in local.iter().chain(anchor) {
        check_reaches(&hashes, head, verifying)?;
    }
    if let Some(kept) = rejected_head_files(path).first() {
        return Err(LedgerError::Broken {
            seq: hashes.len() as u64,
            reason: format!(
                "an earlier head checkpoint no longer matched the chain (entries were removed or rewritten); it is kept as {} - delete that file once you have investigated",
                kept.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
            ),
        });
    }
    Ok(hashes.len() as u64)
}

/// The chain must contain `head`: same hash at the same seq, and nothing cut off after it.
fn check_reaches(
    hashes: &[Hash],
    head: &SignedHead,
    verifying: &VerifyingKey,
) -> Result<(), LedgerError> {
    head.verify_sig(verifying)?;
    if head.seq == 0 {
        return Ok(());
    }
    let len = hashes.len() as u64;
    if head.seq > len {
        return Err(LedgerError::Broken {
            seq: len + 1,
            reason: format!(
                "ledger truncated: a signed head checkpoint is at seq {} but the chain ends at seq {len}",
                head.seq
            ),
        });
    }
    // (verify_sig above already checked that the hash decodes.)
    if hashes[(head.seq - 1) as usize] != to_hash(&head.hash)? {
        return Err(LedgerError::Broken {
            seq: head.seq,
            reason: "entry does not match the signed head checkpoint (history was rewritten)"
                .into(),
        });
    }
    Ok(())
}

/// Check every hash, signature and link; return each entry's hash in order.
fn verify_chain(path: &Path, verifying: &VerifyingKey) -> Result<Vec<Hash>, LedgerError> {
    let entries = read_entries(path)?;
    let mut hashes = Vec::with_capacity(entries.len());
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
        hashes.push(stored);
    }
    Ok(hashes)
}

/// If the last non-empty line of the ledger is unterminated or unparseable (the line
/// `read_entries` tolerates), cut the file back to the end of the last good line.
fn truncate_torn_tail(path: &Path) -> Result<(), LedgerError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    // Start offset of the last non-empty line, and that line's bytes.
    let mut start = 0usize;
    let mut last: Option<(usize, &[u8])> = None;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if !line.iter().all(|b| b.is_ascii_whitespace()) {
            last = Some((start, line));
        }
        start += line.len();
    }
    let Some((offset, line)) = last else {
        return Ok(());
    };
    let terminated = line.ends_with(b"\n");
    if serde_json::from_slice::<Entry>(line).is_ok() {
        if !terminated {
            // A complete entry that only lost its newline (the crash hit between the JSON and the
            // `\n`, or a tool stripped the final newline). `replay_tip` already counted it, so it
            // must be kept: dropping it would break the chain at the next append. Terminate it so
            // the next append starts on its own line.
            let mut f = OpenOptions::new().append(true).open(path)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        return Ok(());
    }
    tracing::warn!(
        path = %path.display(),
        dropped_bytes = bytes.len() - offset,
        "ledger ends with a torn entry from an interrupted append; truncating it"
    );
    let f = OpenOptions::new().write(true).open(path)?;
    f.set_len(offset as u64)?;
    f.sync_all()?;
    Ok(())
}

fn load_or_create_key(path: &Path) -> Result<SigningKey, LedgerError> {
    match std::fs::read(path) {
        Ok(bytes) => return key_from_bytes(&bytes),
        // Only a missing key may be generated. Any other error (permissions, I/O) must
        // surface: replacing the key would invalidate every past signature.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| LedgerError::Rand(e.to_string()))?;

    // Write the key to a temp file, fsync it, then link it into place. The link fails if
    // another process created a key in the meantime, in which case that key wins.
    let tmp = dir.join(format!(".ledger.key.{}.tmp", std::process::id()));
    write_secret(&tmp, &seed)?;
    let placed = match std::fs::hard_link(&tmp, path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        // Filesystems without hard links: fall back to an atomic rename.
        Err(_) => std::fs::rename(&tmp, path).map(|_| true),
    };
    let _ = std::fs::remove_file(&tmp);
    if !placed? {
        return key_from_bytes(&std::fs::read(path)?);
    }
    #[cfg(unix)]
    {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(SigningKey::from_bytes(&seed))
}

fn key_from_bytes(bytes: &[u8]) -> Result<SigningKey, LedgerError> {
    let seed: [u8; 32] = bytes.try_into().map_err(|_| LedgerError::Key)?;
    Ok(SigningKey::from_bytes(&seed))
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), LedgerError> {
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(unix))]
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
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
        let e1 = l
            .append("memory.write", "core", json!({ "fact": "user likes Rust" }))
            .unwrap();
        let e2 = l
            .append(
                "skill.promote",
                "user",
                json!({ "skill": "drafter", "v": 3 }),
            )
            .unwrap();
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
            l.append("memory.write", "core", json!({ "amount": 10 }))
                .unwrap();
            l.append("memory.write", "core", json!({ "amount": 20 }))
                .unwrap();
            verifying = l.verifying;
        }
        // Rewrite the first entry's payload on disk, keeping its old hash/sig.
        let mut lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
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
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"seq\":3,\"kind\":\"c\",\"actor\":\"core\",\"payl")
            .unwrap();
        drop(f);
        // The ledger still loads (not bricked) and verifies its complete prefix.
        let l = Ledger::open(dir.path()).unwrap();
        assert_eq!(l.head().0, 2);
        assert_eq!(verify_file(&path, &verifying).unwrap(), 2);
    }

    fn drop_last_lines(dir: &Path, n: usize) {
        let path = dir.join("ledger.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let kept = lines[..lines.len() - n].join("\n") + "\n";
        std::fs::write(&path, kept).unwrap();
    }

    #[test]
    fn deleting_entries_from_the_end_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(dir.path()).unwrap();
        for i in 0..5 {
            l.append("t", "a", serde_json::json!({ "i": i })).unwrap();
        }
        assert_eq!(l.verify().unwrap(), 5);
        drop_last_lines(dir.path(), 2);
        match l.verify() {
            Err(LedgerError::Broken { reason, .. }) => {
                assert!(reason.contains("truncated"), "{reason}")
            }
            other => panic!("truncation not detected: {other:?}"),
        }
    }

    #[test]
    fn truncation_while_stopped_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        {
            let l = Ledger::open(dir.path()).unwrap();
            for i in 0..5 {
                l.append("t", "a", serde_json::json!({ "i": i })).unwrap();
            }
        } // dropped: head is current (seq 5)
        drop_last_lines(dir.path(), 2);
        // Restart and carry on: the evidence must not be overwritten.
        let l = Ledger::open(dir.path()).unwrap();
        l.append("t", "a", serde_json::json!({ "after": true }))
            .unwrap();
        let all = l.read_all().unwrap();
        let event = all
            .iter()
            .find(|e| e.kind == "ledger.truncation_detected")
            .expect("truncation recorded in the chain");
        let payload: serde_json::Value = serde_json::from_str(event.payload.get()).unwrap();
        assert_eq!(payload["head"]["seq"], 5);
        assert_eq!(payload["chain_seq"], 3);
        assert!(l.verify().is_err(), "verify must keep failing");
        assert!(l.verify().is_err(), "even after the head was refreshed");
        // Once investigated, removing the kept head clears the alarm; the event stays.
        for p in rejected_head_files(&dir.path().join("ledger.jsonl")) {
            std::fs::remove_file(p).unwrap();
        }
        assert_eq!(l.verify().unwrap(), all.len() as u64);
    }

    #[test]
    fn a_malformed_head_is_reported_as_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(dir.path()).unwrap();
        l.append("t", "a", serde_json::json!({})).unwrap();
        let path = dir.path().join("ledger.jsonl");
        let bad = SignedHead {
            seq: 1,
            hash: "zz".into(),
            sig: "not hex".into(),
        };
        assert!(matches!(
            verify_file_with_anchor(&path, &l.verifying, Some(&bad)),
            Err(LedgerError::Broken { .. })
        ));
    }

    #[test]
    fn a_removed_head_is_recreated_and_recorded_in_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("t", "a", serde_json::json!({})).unwrap();
            l.append("t", "a", serde_json::json!({})).unwrap();
        }
        std::fs::remove_file(dir.path().join("ledger.head")).unwrap();
        let l = Ledger::open(dir.path()).unwrap();
        let all = l.read_all().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[2].kind, "ledger.head_created");
        assert_eq!(l.verify().unwrap(), 3);
        // A fresh, empty ledger needs no such entry.
        let empty = tempfile::tempdir().unwrap();
        assert!(Ledger::open(empty.path())
            .unwrap()
            .read_all()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_external_anchor_catches_truncation_even_without_the_local_head() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(dir.path()).unwrap();
        for _ in 0..3 {
            l.append("t", "a", serde_json::json!({})).unwrap();
        }
        let saved = l.signed_head().unwrap(); // kept off the machine
        l.append("t", "a", serde_json::json!({})).unwrap();
        let path = dir.path().join("ledger.jsonl");
        // The chain grew past the anchor: fine.
        assert_eq!(
            verify_file_with_anchor(&path, &l.verifying, Some(&saved)).unwrap(),
            4
        );
        // Cut back past the anchor and remove the local head: only the anchor can tell.
        drop_last_lines(dir.path(), 2);
        std::fs::remove_file(dir.path().join("ledger.head")).unwrap();
        assert_eq!(verify_file(&path, &l.verifying).unwrap(), 2);
        assert!(verify_file_with_anchor(&path, &l.verifying, Some(&saved)).is_err());
    }

    #[test]
    fn a_forged_or_mismatched_head_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(dir.path()).unwrap();
        l.append("t", "a", serde_json::json!({})).unwrap();
        let path = dir.path().join("ledger.jsonl");
        let other = tempfile::tempdir().unwrap();
        let foreign = Ledger::open(other.path()).unwrap();
        foreign.append("t", "a", serde_json::json!({})).unwrap();
        // Signed by a different key.
        assert!(verify_file_with_anchor(
            &path,
            &l.verifying,
            Some(&foreign.signed_head().unwrap())
        )
        .is_err());
        // Valid signature over a hash that isn't in this chain.
        let mut wrong = l.signed_head().unwrap();
        wrong.hash = foreign.signed_head().unwrap().hash;
        assert!(verify_file_with_anchor(&path, &l.verifying, Some(&wrong)).is_err());
    }

    #[test]
    fn verifying_while_appending_never_reports_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let l = std::sync::Arc::new(Ledger::open(dir.path()).unwrap());
        let writer = {
            let l = l.clone();
            std::thread::spawn(move || {
                for i in 0..400 {
                    l.append("t", "a", serde_json::json!({ "i": i })).unwrap();
                }
            })
        };
        for _ in 0..40 {
            l.verify().expect("verify during appends");
        }
        writer.join().unwrap();
        assert_eq!(l.verify().unwrap(), 400);
    }

    #[test]
    fn tail_matches_the_full_read_for_any_n() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(dir.path()).unwrap();
        // Big payloads so the backwards read crosses several 64 KiB chunks.
        for i in 0..300 {
            l.append(
                "t",
                "a",
                serde_json::json!({ "i": i, "pad": "é".repeat(400) }),
            )
            .unwrap();
        }
        let all = read_entries(&l.path).unwrap();
        for n in [0, 1, 2, 50, 299, 300, 1000] {
            let t = l.tail(n).unwrap();
            let want = &all[all.len().saturating_sub(n)..];
            assert_eq!(t.len(), want.len(), "n={n}");
            assert!(t.iter().zip(want).all(|(a, b)| a.seq == b.seq), "n={n}");
        }
        // A torn trailing line is skipped, as in the full read.
        let mut f = OpenOptions::new().append(true).open(&l.path).unwrap();
        f.write_all(b"{\"seq\":301,\"ts").unwrap();
        let t = l.tail(3).unwrap();
        assert_eq!(t.last().unwrap().seq, 300);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn complete_last_entry_without_newline_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("t", "a", serde_json::json!({"n": 1})).unwrap();
            l.append("t", "a", serde_json::json!({"n": 2})).unwrap();
        }
        // Strip the final newline: the last entry is complete but unterminated.
        let path = dir.path().join("ledger.jsonl");
        let mut bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.pop(), Some(b'\n'));
        std::fs::write(&path, &bytes).unwrap();
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("t", "a", serde_json::json!({"n": 3})).unwrap();
        }
        let l = Ledger::open(dir.path()).unwrap();
        assert_eq!(
            l.verify().unwrap(),
            3,
            "the unterminated entry must survive"
        );
    }

    #[test]
    fn torn_tail_is_truncated_so_later_appends_stay_valid() {
        let dir = temp();
        let path = dir.path().join("ledger.jsonl");
        {
            let l = Ledger::open(dir.path()).unwrap();
            l.append("a", "core", json!(1)).unwrap();
            l.append("b", "core", json!(2)).unwrap();
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"seq\":3,\"kind\":\"c\",\"act").unwrap();
        drop(f);
        {
            let l = Ledger::open(dir.path()).unwrap();
            assert_eq!(l.head().0, 2);
            l.append("c", "core", json!(3)).unwrap();
            l.append("d", "core", json!(4)).unwrap();
        }
        let l = Ledger::open(dir.path()).unwrap();
        assert_eq!(l.verify().unwrap(), 4);
        assert_eq!(l.read_all().unwrap().len(), 4);
        assert!(std::fs::read(&path).unwrap().ends_with(b"\n"));
    }

    #[test]
    fn existing_key_is_reused_and_unreadable_key_is_not_replaced() {
        let dir = temp();
        let pk = Ledger::open(dir.path()).unwrap().pubkey_hex();
        assert_eq!(Ledger::open(dir.path()).unwrap().pubkey_hex(), pk);
        // No temp files are left next to the key.
        let names: Vec<_> = std::fs::read_dir(dir.path().join("keys"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("ledger.key")]);

        // A read error other than "not found" must fail rather than mint a new key.
        // A directory in place of the key file yields such an error on every platform.
        let other = temp();
        std::fs::create_dir_all(other.path().join("keys").join("ledger.key")).unwrap();
        assert!(Ledger::open(other.path()).is_err());
        assert!(other.path().join("keys").join("ledger.key").is_dir());
    }

    #[test]
    fn exported_pubkey_verifies_the_chain_offline() {
        let dir = temp();
        let l = Ledger::open(dir.path()).unwrap();
        l.append("a", "core", json!(1)).unwrap();
        l.append("b", "core", json!(2)).unwrap();
        // The published public key (hex) reconstructs and verifies the signed file.
        let vk = verifying_key_from_hex(&l.pubkey_hex()).unwrap();
        assert_eq!(
            verify_file(&dir.path().join("ledger.jsonl"), &vk).unwrap(),
            2
        );
        // Bad hex is an error, never a panic.
        assert!(verifying_key_from_hex("not-hex").is_err());
    }

    #[test]
    fn revert_is_appended_not_erased() {
        let dir = temp();
        let l = Ledger::open(dir.path()).unwrap();
        let bad = l
            .append(
                "skill.mutate",
                "skill:drafter@4",
                json!({ "regression": true }),
            )
            .unwrap();
        let rev = l.revert("user", &bad.hash, "metric dropped").unwrap();
        assert_eq!(rev.kind, "revert");
        let payload: serde_json::Value = serde_json::from_str(rev.payload.get()).unwrap();
        assert_eq!(payload["target"], bad.hash);
        assert_eq!(l.verify().unwrap(), 2); // both entries remain, chain intact
    }
}
