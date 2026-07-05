//! The memory broker.
//!
//! One embedded SQLite file *is* the brain on disk: facts with FTS5 for keyword
//! search and a stored embedding for semantic search, all in one WAL file that
//! survives the core sleeping to zero. [`Memory::recall`] runs both searches and
//! fuses them with Reciprocal Rank Fusion, so a paraphrased query with no shared
//! words still surfaces the right memory - exactly where a keyword-only agent
//! returns nothing.
//!
//! Every write, forget, and consolidation is recorded in the [`Ledger`] first, so
//! the brain's history is signed and reversible.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use engram_core::{now_ms, Ledger, Taint};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

use crate::embed::{cosine, from_bytes, hamming, quantize_binary, to_bytes, Embedder};
use crate::region::Region;
use engram_core::{Scope, ScopeCtx};

/// How many candidates each search arm contributes before fusion.
const ARM_LIMIT: usize = 64;
/// The coarse (binary) pass keeps the best this-many candidates by Hamming distance before the exact
/// cosine rerank. It scans EVERY live in-scope vector (no salience cap), so an old, low-importance,
/// semantically-perfect memory is never silently dropped - the completeness fix.
const COARSE_K: usize = 256;
/// Below this many live in-scope candidates, recall SKIPS the binary coarse pre-filter and ranks
/// them all by exact cosine. The binary code is a weak discriminator for the sparse default embedder
/// (see `quantize_binary`), so at typical scale the coarse truncation could drop a genuine
/// paraphrase; exact cosine over a few thousand 256-float vectors is cheap and provably complete.
/// The coarse pass only re-engages past this threshold, to bound cost on very large rings.
const EXACT_SCAN_MAX: usize = 5000;
/// Chunk size for fetching candidate embeddings, so the `id IN (...)` fetch stays within SQLite's
/// bound-variable limit even when the whole (un-truncated) in-scope ring is a candidate.
const EXACT_FETCH_BATCH: usize = 500;
/// MMR diversity/relevance trade-off for the semantic arm: relevance-dominant, but enough novelty
/// pressure to break up near-duplicate passages.
const MMR_LAMBDA: f32 = 0.7;
/// Reciprocal Rank Fusion constant (standard default).
const RRF_K: f32 = 60.0;
/// Bound on how many stale/low-importance candidate rows one reflection tick considers, across all
/// region+scope groups combined - keeps [`Memory::reflection_candidates`]'s extra query cheap
/// regardless of brain size.
const REFLECTION_CANDIDATE_LIMIT: usize = 300;

const COLS: &str =
    "id,region,text,importance,taint,tier,source,metadata,content_hash,ledger_seq,created_ms,last_access_ms,access_count,scope_kind,scope_id,actor,valid_from_ms,valid_until_ms,tree_level";

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("ledger: {0}")]
    Ledger(#[from] engram_core::LedgerError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// The injection guard refused to promote an untrusted-provenance memory into the trusted
    /// user-global ring. A deliberate rule (scraped/attacker content must never become a global fact
    /// about the user), so it carries a human message the API/UI can surface as a 4xx rather than the
    /// cryptic `sqlite: ...` this used to masquerade as.
    #[error("untrusted memories cannot be promoted to the global ring")]
    UntrustedPromotion,
}

type Result<T> = std::result::Result<T, MemoryError>;

/// A request to store a memory.
pub struct WriteReq {
    pub region: Region,
    pub text: String,
    /// 0.0-1.0; higher resists forgetting and consolidation demotion.
    pub importance: f32,
    pub taint: Taint,
    pub source: Option<String>,
    pub metadata: serde_json::Value,
    /// Who is writing this (for the audit trail), e.g. "core" or "skill:drafter@3".
    pub actor: String,
    /// Which world this memory belongs to (user-global by default). Recall is ringed by scope.
    pub scope: Scope,
    /// RAPTOR-style tree-summary groundwork (see docs/MEMORY-UPGRADE-PLAN.md §4): 0 for an ordinary,
    /// directly-witnessed fact; 1 for a synthesized fact the reflection pass wrote from other facts.
    /// Not wired to any real multi-level tree yet — this only makes "is this a synthesis" a real
    /// column instead of requiring a metadata-only convention, so a future second-level reflection
    /// pass doesn't need a second schema migration.
    pub tree_level: i64,
}

impl WriteReq {
    pub fn new(region: Region, text: impl Into<String>) -> Self {
        WriteReq {
            region,
            text: text.into(),
            importance: 0.5,
            taint: Taint::Trusted,
            source: None,
            metadata: serde_json::Value::Null,
            actor: "core".into(),
            scope: Scope::user(),
            tree_level: 0,
        }
    }
    pub fn importance(mut self, v: f32) -> Self {
        self.importance = v.clamp(0.0, 1.0);
        self
    }
    pub fn taint(mut self, t: Taint) -> Self {
        self.taint = t;
        self
    }
    pub fn source(mut self, s: impl Into<String>) -> Self {
        self.source = Some(s.into());
        self
    }
    pub fn actor(mut self, a: impl Into<String>) -> Self {
        self.actor = a.into();
        self
    }
    /// Bind this write to a scope (user-global, a project, or a session).
    pub fn scope(mut self, s: Scope) -> Self {
        self.scope = s;
        self
    }
    pub fn metadata(mut self, m: serde_json::Value) -> Self {
        self.metadata = m;
        self
    }
    /// Mark this write as a reflection-pass synthesis (see [`Record::tree_level`]). Ordinary writes
    /// never call this — it defaults to 0.
    pub fn tree_level(mut self, level: i64) -> Self {
        self.tree_level = level;
        self
    }
}

/// A stored memory (embedding omitted - it is an internal index, not content).
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: i64,
    pub region: String,
    pub text: String,
    pub importance: f32,
    pub taint: String,
    pub tier: String,
    pub source: Option<String>,
    pub metadata: serde_json::Value,
    pub content_hash: String,
    pub ledger_seq: Option<i64>,
    pub created_ms: i64,
    pub last_access_ms: i64,
    pub access_count: i64,
    /// Which world this memory lives in - `user` (global), `project`, or `session`.
    pub scope_kind: String,
    /// The project/session id for a scoped memory; empty for user-global.
    pub scope_id: String,
    /// Who wrote this memory - `user`, `core`, a skill id, or (per the durable-named-agent model)
    /// the agent's own name, e.g. `agent:Atlas`. Attribution, not isolation: an agent's memory
    /// still lives in and is recalled from the normal project/user ring like anyone else's -
    /// this only makes "which agent said this" a filterable fact instead of requiring a ledger
    /// cross-reference for something the fact itself should just say.
    pub actor: String,
    /// When this fact became true (defaults to `created_ms` for a fresh write). Bi-temporal
    /// versioning: `superseded_by` is still the UI-facing "current truth" pointer, unchanged; these
    /// two columns are strictly additive and only consulted by [`Memory::recall_as_of`].
    pub valid_from_ms: i64,
    /// When this fact stopped being true - `None` while it's still current. Set by
    /// [`Memory::supersede`] on the OLD row at the moment a newer fact replaces it.
    pub valid_until_ms: Option<i64>,
    /// RAPTOR-style tree-summary groundwork (see docs/MEMORY-UPGRADE-PLAN.md §4): 0 for an
    /// ordinary, directly-witnessed fact; 1 for a synthesized fact the reflection pass
    /// (`reflection.rs`) wrote from a small co-scoped group of other facts (whose ids/ledger
    /// sequences live in `metadata.source_ids`/`source_seqs`). No row has ever had a *parent*
    /// summary yet, so there is no `parent_id` field here - only the schema column exists
    /// (via migration), reserved for a future second-level reflection pass without a second
    /// migration.
    pub tree_level: i64,
}

/// A detected-but-unconfirmed contradiction: `candidate_text` MAY supersede the current fact
/// `old_id`, per the model's cited `reason` - never applied silently (the mandatory-confirmation
/// design: see docs/MEMORY-UPGRADE-PLAN.md §5). Resolved by a human via [`Memory::resolve_supersession`].
#[derive(Debug, Clone, Serialize)]
pub struct PendingSupersession {
    pub id: i64,
    pub old_id: i64,
    pub candidate_text: String,
    pub reason: String,
    pub region: String,
    pub scope_kind: String,
    pub scope_id: String,
    pub actor: String,
    pub created_ms: i64,
}

/// A recall result with its fused score and the rank each arm gave it (so the UI can
/// show *why* a memory surfaced - keyword, semantic, or both).
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub record: Record,
    pub score: f32,
    pub keyword_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
}

/// Per-tier / per-region counts for the Memory Atlas view.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Stats {
    pub total: i64,
    pub by_region: HashMap<String, i64>,
    pub by_tier: HashMap<String, i64>,
    /// Who wrote each live memory - `user`/`core`/a skill id/`agent:<name>`. Lets a durable named
    /// agent's own contribution to a brain (or, via `stats_for_scope`, to one project) be seen
    /// directly instead of cross-referencing the ledger for every fact one at a time.
    pub by_actor: HashMap<String, i64>,
    /// Live rows embedded via a degraded fallback (see `needs_reembed` on `facts`) - not yet
    /// re-embedded in the configured model's real space, so mis-ranked until the repair pass runs.
    pub needs_reembed: i64,
}

/// Accumulates recall-hit access-count bumps between ledger flushes (see `Memory::record_access`).
#[derive(Default)]
struct AccessBatch {
    ids: Vec<i64>,
    window_start_ms: i64,
}

/// The brain on disk.
pub struct Memory {
    conn: Mutex<Connection>,
    embedder: Arc<dyn Embedder>,
    ledger: Arc<Ledger>,
    /// Batches recall-hit access-count bumps into one signed `memory.access_batch` ledger entry
    /// per minute of activity, so the field driving `consolidate()`'s demotion decision is part of
    /// the signed history without ledgering every single recall touch individually (a single
    /// recall call can touch dozens of rows - one entry per hit would be pure noise).
    access_batch: Mutex<AccessBatch>,
}

impl Memory {
    /// Open (or create) the brain at `path`, wired to `embedder` and `ledger`.
    pub fn open(
        path: impl AsRef<Path>,
        embedder: Arc<dyn Embedder>,
        ledger: Arc<Ledger>,
    ) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        init_schema(&conn)?;
        let mem = Memory {
            conn: Mutex::new(conn),
            embedder,
            ledger,
            access_batch: Mutex::new(AccessBatch::default()),
        };
        mem.migrate_embedding_space()?;
        mem.backfill_binary()?;
        Ok(mem)
    }

    /// Record that a recall hit touched `id`, batching into one ledger entry per minute of
    /// activity (flushed lazily on the next access once the window elapses - there is no
    /// background timer, so a batch open when the process exits is lost; access-count is a soft
    /// salience signal feeding only the reversible warm/cold demotion, not content, so losing at
    /// most a minute of it on an ungraceful shutdown is an accepted, deliberately small gap).
    fn record_access(&self, id: i64) {
        let mut batch = self.access_batch.lock().expect("access-batch mutex poisoned");
        let now = now_ms() as i64;
        if batch.ids.is_empty() {
            batch.window_start_ms = now;
        }
        batch.ids.push(id);
        if now - batch.window_start_ms >= 60_000 {
            let count = batch.ids.len();
            let ids = std::mem::take(&mut batch.ids);
            let window_start_ms = batch.window_start_ms;
            batch.window_start_ms = now;
            drop(batch);
            let _ = self.ledger.append(
                "memory.access_batch",
                "core",
                json!({ "ids": ids, "count": count, "window_start_ms": window_start_ms, "window_end_ms": now }),
            );
        }
    }

    /// Populate `embedding_bin` for rows written before the binary index existed, computing each
    /// code from the stored `embedding` (no re-embedding needed). Bounded and idempotent: once every
    /// row has a code this is a cheap no-op. (P5 makes this resumable/off-boot for very large brains.)
    ///
    /// Deliberately unledgered, unlike every content mutation in this file (the I1 invariant):
    /// `embedding_bin` is purely DERIVED state, recomputable at any time from `embedding` (the
    /// source of truth) with no external input - the same class of exemption as the sidecar index
    /// `reindex_binary` rebuilds on demand. There is nothing here a signed history could attest to
    /// beyond "this cache was refreshed," which the cache's own presence already shows.
    fn backfill_binary(&self) -> Result<()> {
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        let rows: Vec<(i64, Vec<u8>)> = {
            let mut stmt =
                conn.prepare("SELECT id, embedding FROM facts WHERE embedding_bin IS NULL")?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
            let mut v = Vec::new();
            for row in mapped {
                v.push(row?);
            }
            v
        };
        if rows.is_empty() {
            return Ok(());
        }
        let tx = conn.transaction()?;
        for (id, blob) in &rows {
            let bin = quantize_binary(&from_bytes(blob));
            tx.execute(
                "UPDATE facts SET embedding_bin = ?1 WHERE id = ?2",
                params![bin, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Re-embed every stored memory when the active embedding model/space changes. Vectors
    /// from a different model live in an incomparable space, so without this a query under
    /// the new model would silently fail to match old memories. A no-op when the space is
    /// unchanged; on a genuine switch it re-embeds in one transaction and records it.
    fn migrate_embedding_space(&self) -> Result<()> {
        let current = format!("{}:{}", self.embedder.name(), self.embedder.dim());
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'embed_space'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if stored.as_deref() == Some(current.as_str()) {
            return Ok(());
        }
        // Gather EVERY row that needs re-embedding (collect first so the statement is dropped before
        // we open the write transaction). Tombstoned (deleted = 1) rows are included on purpose: a
        // later restore() only flips `deleted` back to 0 and does NOT re-embed, so skipping them here
        // would leave a restored memory carrying an OLD-embedding-space vector forever - it would
        // rank randomly in recall with no repair path. They are few, so migrating them is cheap.
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn.prepare("SELECT id, text FROM facts")?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            let mut v = Vec::new();
            for row in mapped {
                v.push(row?);
            }
            v
        };
        // First-ever open with nothing stored: just stamp the space, nothing to migrate.
        if stored.is_none() && rows.is_empty() {
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('embed_space', ?1)",
                params![current],
            )?;
            return Ok(());
        }
        // Audit BEFORE mutating (ledger-first, like remember) and propagate any ledger
        // failure, so a re-embed can never happen silently or go unrecorded.
        self.ledger.append(
            "memory.reembed",
            "core",
            json!({ "space": current, "previous": stored, "rows": rows.len() }),
        )?;
        let tx = conn.transaction()?;
        for (id, text) in &rows {
            // Re-embedding into a new space invalidates the binary code too - recompute both.
            let v = self.embedder.embed(text);
            tx.execute(
                "UPDATE facts SET embedding = ?1, embedding_bin = ?2 WHERE id = ?3",
                params![to_bytes(&v), quantize_binary(&v), id],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('embed_space', ?1)",
            params![current],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Store a memory: embed it, record it in the ledger, then persist it.
    pub fn remember(&self, req: WriteReq) -> Result<Record> {
        let (embedding, needs_reembed) = self.embedder.embed_checked(&req.text);
        let blob = to_bytes(&embedding);
        let bin = quantize_binary(&embedding);
        let content_hash = blake3::hash(req.text.as_bytes()).to_hex().to_string();
        let now = now_ms() as i64;
        let taint = taint_str(req.taint);

        // Dedup: re-learning a fact that already exists VERBATIM in the same region must not pile up
        // a duplicate row (that is how the table grows unbounded). Instead bump its importance (to
        // the max of old/new) and access count - reinforcing what is repeatedly seen - and return it.
        // Dedup-and-insert under a SINGLE held connection lock so the check and the insert are
        // atomic. The earlier version released the lock between the SELECT and the INSERT, so two
        // concurrent remember() of the same (region, content_hash) could both miss the row and both
        // INSERT a duplicate (a TOCTOU). Holding the one Mutex<Connection> across both closes it.
        // ledger.append() runs while the lock is held, which is safe: it never re-enters the memory
        // connection, so there is no lock cycle.
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        // Dedup is scope-aware: the SAME fact text in two different rings (e.g. a note that is
        // both a user-global preference and a project fact) is two distinct rows, so bumping one
        // never reaches across the ring boundary.
        // The existing row's taint is part of the decision: dedup must respect provenance, not just
        // text. A trusted re-assertion of a previously-untrusted fact should UPGRADE it (so it
        // becomes visible to trusted recall and consciousness), and an untrusted write must NEVER
        // inflate the salience of a trusted row (tainted activity cannot reinforce trusted memory).
        let existing: Option<(i64, f64, String)> = conn
            .query_row(
                "SELECT id, importance, taint FROM facts WHERE region = ?1 AND content_hash = ?2 \
                 AND scope_kind = ?3 AND scope_id = ?4 AND deleted = 0 AND superseded_by IS NULL LIMIT 1",
                params![req.region.as_str(), content_hash, req.scope.kind.as_str(), req.scope.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((id, old_imp, old_taint)) = existing {
            let existing_untrusted = old_taint != "trusted";
            let writing_trusted = !req.taint.is_untrusted();
            if existing_untrusted && !writing_trusted {
                // Untrusted duplicate of an untrusted row: reinforce as before (bump importance +
                // access). Ledger-first (append then mutate), matching remember/forget.
                let new_imp = (old_imp as f32).max(req.importance);
                self.ledger.append(
                    "memory.write",
                    &req.actor,
                    json!({ "region": req.region.as_str(), "content_hash": content_hash, "deduped": true }),
                )?;
                conn.execute(
                    "UPDATE facts SET importance = ?1, access_count = access_count + 1, last_access_ms = ?2 WHERE id = ?3",
                    params![new_imp as f64, now, id],
                )?;
            } else if existing_untrusted && writing_trusted {
                // A TRUSTED assertion of a fact first captured Untrusted: upgrade the row's taint (a
                // distinct, auditable event) so the trusted assertion isn't silently discarded and the
                // fact becomes eligible for trusted recall / consciousness. Also bump importance.
                let new_imp = (old_imp as f32).max(req.importance);
                self.ledger.append(
                    "memory.trust",
                    &req.actor,
                    json!({ "region": req.region.as_str(), "content_hash": content_hash, "from": old_taint, "to": "trusted" }),
                )?;
                conn.execute(
                    "UPDATE facts SET taint = 'trusted', importance = ?1, access_count = access_count + 1, last_access_ms = ?2 WHERE id = ?3",
                    params![new_imp as f64, now, id],
                )?;
            } else if !existing_untrusted && !writing_trusted {
                // An UNTRUSTED write matching a TRUSTED row: dedup (no duplicate row) but do NOT let
                // tainted activity mutate the trusted row's importance/access. Just record the touch.
                self.ledger.append(
                    "memory.write",
                    &req.actor,
                    json!({ "region": req.region.as_str(), "content_hash": content_hash, "deduped": true, "untrusted_touch": true }),
                )?;
            } else {
                // Both trusted: reinforce (bump importance + access).
                let new_imp = (old_imp as f32).max(req.importance);
                self.ledger.append(
                    "memory.write",
                    &req.actor,
                    json!({ "region": req.region.as_str(), "content_hash": content_hash, "deduped": true }),
                )?;
                conn.execute(
                    "UPDATE facts SET importance = ?1, access_count = access_count + 1, last_access_ms = ?2 WHERE id = ?3",
                    params![new_imp as f64, now, id],
                )?;
            }
            if let Some(rec) = get_record(&conn, id)? {
                return Ok(rec);
            }
            // (Effectively unreachable: we just updated this row.) Fall through to a fresh insert,
            // still holding the lock so it stays atomic.
        }

        let entry = self.ledger.append(
            "memory.write",
            &req.actor,
            json!({
                "region": req.region.as_str(),
                "content_hash": content_hash,
                "importance": req.importance,
                "taint": taint,
                "source": req.source,
            }),
        )?;

        let meta_str = serde_json::to_string(&req.metadata)?;
        // One transaction so the row and its FTS index are all-or-nothing - a failure
        // can never leave the fact searchable-but-missing or present-but-unsearchable.
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO facts(region,text,importance,taint,tier,source,metadata,embedding,content_hash,ledger_seq,created_ms,last_access_ms,scope_kind,scope_id,embedding_bin,needs_reembed,actor,valid_from_ms,tree_level) \
             VALUES(?1,?2,?3,?4,'warm',?5,?6,?7,?8,?9,?10,?10,?11,?12,?13,?14,?15,?10,?16)",
            params![
                req.region.as_str(),
                req.text,
                req.importance as f64,
                taint,
                req.source,
                meta_str,
                blob,
                content_hash,
                entry.seq as i64,
                now,
                req.scope.kind.as_str(),
                req.scope.id,
                bin,
                needs_reembed as i64,
                req.actor,
                req.tree_level,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO facts_fts(rowid, text) VALUES(?1, ?2)",
            params![id, req.text],
        )?;
        tx.commit()?;

        Ok(Record {
            id,
            region: req.region.as_str().to_string(),
            text: req.text,
            importance: req.importance,
            taint: taint.to_string(),
            tier: "warm".to_string(),
            source: req.source,
            metadata: req.metadata,
            content_hash,
            ledger_seq: Some(entry.seq as i64),
            created_ms: now,
            last_access_ms: now,
            access_count: 0,
            scope_kind: req.scope.kind.as_str().to_string(),
            scope_id: req.scope.id,
            actor: req.actor,
            valid_from_ms: now,
            valid_until_ms: None,
            tree_level: req.tree_level,
        })
    }

    /// Hybrid recall across the WHOLE brain (every scope): BM25 keyword + vector semantic,
    /// fused by RRF. `regions` empty means every region. Includes ALL provenance (even untrusted)
    /// — for transparency / audit / the Atlas. For model-facing recall use [`Memory::recall_scoped`]
    /// so the user/project/session rings are respected and one project can't read another's work.
    pub fn recall(&self, query: &str, regions: &[Region], k: usize) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, false, &ScopeCtx::any(), None)
    }

    /// Whole-brain recall EXCLUDING untrusted-provenance memories. See [`Memory::recall_trusted_scoped`]
    /// for the ringed, model-facing variant.
    pub fn recall_trusted(&self, query: &str, regions: &[Region], k: usize) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, true, &ScopeCtx::any(), None)
    }

    /// Ringed recall: restricted to the union of the rings in `scope` (user ∪ active project ∪
    /// active session), the more-specific ring boosted on ties. Includes all provenance.
    pub fn recall_scoped(
        &self,
        query: &str,
        regions: &[Region],
        k: usize,
        scope: &ScopeCtx,
    ) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, false, scope, None)
    }

    /// Ringed, TRUSTED-only recall - what a model should get as grounded context: only the rings
    /// that apply to this turn, and never untrusted-provenance memory (the memory-poisoning guard).
    pub fn recall_trusted_scoped(
        &self,
        query: &str,
        regions: &[Region],
        k: usize,
        scope: &ScopeCtx,
    ) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, true, scope, None)
    }

    /// "What did I believe as of `as_of_ms`" - a bi-temporal time-travel query, additive alongside
    /// every other recall variant (which are all unaffected: they pass `as_of_ms: None`, matching
    /// the exact prior behavior of filtering on `superseded_by IS NULL`). Trusted-only and ringed,
    /// matching the model-facing/user-inspection family this belongs to (`recall_trusted_scoped`).
    pub fn recall_as_of(
        &self,
        query: &str,
        regions: &[Region],
        k: usize,
        scope: &ScopeCtx,
        as_of_ms: i64,
    ) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, true, scope, Some(as_of_ms))
    }

    fn recall_inner(
        &self,
        query: &str,
        regions: &[Region],
        k: usize,
        trusted_only: bool,
        scope: &ScopeCtx,
        as_of_ms: Option<i64>,
    ) -> Result<Vec<Hit>> {
        let taint_clause = |prefix: &str| {
            if trusted_only {
                format!(" AND {prefix}taint = 'trusted'")
            } else {
                String::new()
            }
        };
        // Bi-temporal validity: the default (as_of_ms: None) path is IDENTICAL to before this
        // feature existed (`superseded_by IS NULL`) - recall_as_of is the only caller that ever
        // passes Some. `t` is an i64 timestamp (never user-controlled text), safe to interpolate
        // the same way `region_clause` already inlines its fixed enum strings.
        let validity_clause = |prefix: &str| match as_of_ms {
            None => format!("{prefix}superseded_by IS NULL"),
            Some(t) => format!(
                "{prefix}valid_from_ms <= {t} AND ({prefix}valid_until_ms IS NULL OR {prefix}valid_until_ms > {t})"
            ),
        };
        let q_emb = self.embedder.embed(query);
        let conn = self.conn.lock().expect("memory mutex poisoned");

        // --- keyword arm (BM25; lower is better) ---
        let mut keyword: Vec<i64> = Vec::new();
        if let Some(match_q) = build_match(query) {
            let (scope_sql, scope_binds) = scope_clause("f.", scope);
            let sql = format!(
                "SELECT facts_fts.rowid FROM facts_fts \
                 JOIN facts f ON f.id = facts_fts.rowid \
                 WHERE facts_fts MATCH ? AND f.deleted = 0 AND {validity} AND {region}{taint} AND {scope} \
                 ORDER BY bm25(facts_fts) LIMIT ?",
                validity = validity_clause("f."),
                region = region_clause("f.", regions),
                taint = taint_clause("f."),
                scope = scope_sql,
            );
            // Bind order follows placeholder order in the SQL: MATCH ?, then scope ids, then LIMIT ?.
            let mut binds: Vec<Value> = Vec::with_capacity(scope_binds.len() + 2);
            binds.push(Value::Text(match_q));
            for id in scope_binds {
                binds.push(Value::Text(id));
            }
            binds.push(Value::Integer(ARM_LIMIT as i64));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(binds), |r| r.get::<_, i64>(0))?;
            for id in rows {
                keyword.push(id?);
            }
        }

        // --- semantic arm: exact-cosine recall over ALL live in-scope vectors, with a binary coarse
        // pre-filter only at large scale ---
        // The binary code is a WEAK discriminator for the sparse default embedder (a short text bumps
        // only a handful of the 256 dims), so using Hamming as the sole gate lets short unrelated rows
        // crowd out a genuine paraphrase once a ring exceeds the truncation. So: while a ring holds at
        // most EXACT_SCAN_MAX live in-scope rows (the common case - weeks of episodic capture, a
        // several-hundred-chunk document), skip the coarse stage entirely and rank ALL of them by
        // exact cosine - provably complete, and 256-float dot products over a few thousand rows are
        // trivial. Only past that threshold do we fall back to the coarse Hamming pass to bound cost,
        // and even then we (a) center-quantize (see quantize_binary) for a better ordering and
        // (b) break Hamming ties by recency (id desc), so truncation prefers newer rows over the old
        // insertion-order (oldest-first) bias, then exact-cosine-rerank the survivors.
        let q_bin = quantize_binary(&q_emb);
        let (sem_scope_sql, sem_scope_binds) = scope_clause("", scope);
        let coarse_sql = format!(
            "SELECT id, embedding_bin FROM facts WHERE deleted = 0 AND {validity} AND {region}{taint} AND {scope}",
            validity = validity_clause(""),
            region = region_clause("", regions),
            taint = taint_clause(""),
            scope = sem_scope_sql,
        );
        let mut coarse: Vec<(u32, i64)> = {
            let mut stmt = conn.prepare(&coarse_sql)?;
            let binds: Vec<Value> = sem_scope_binds.into_iter().map(Value::Text).collect();
            let rows = stmt.query_map(params_from_iter(binds), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
            })?;
            let mut out: Vec<(u32, i64)> = Vec::new();
            for row in rows {
                let (id, bin) = row?;
                // A missing code (shouldn't happen after backfill) is treated as the best distance,
                // so a row is never dropped for lack of an index - completeness over micro-speed.
                let dist = bin.map(|b| hamming(&q_bin, &b)).unwrap_or(0);
                out.push((dist, id));
            }
            out
        };
        if coarse.len() > EXACT_SCAN_MAX {
            // Large ring: coarse pre-filter. Tie-break by id descending (recency) so the truncation
            // no longer systematically favours the oldest rows.
            coarse.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
            coarse.truncate(COARSE_K);
        }
        // else: keep every candidate; exact cosine below is the sole (complete) ranker.
        // Stage B: exact cosine over the (coarse or full) candidates, then MMR to diversify - so
        // near-duplicate passages (e.g. overlapping document chunks) don't crowd out other relevant
        // memories.
        let mut sims: Vec<(i64, f32, Vec<f32>)> = Vec::with_capacity(coarse.len());
        if !coarse.is_empty() {
            let ids: Vec<i64> = coarse.iter().map(|(_, id)| *id).collect();
            // Fetch embeddings in bounded batches: without the coarse pre-filter the candidate set is
            // the whole in-scope ring (up to EXACT_SCAN_MAX), which can exceed SQLite's bound-variable
            // limit for a single `IN (...)`, so chunk the id list.
            for chunk in ids.chunks(EXACT_FETCH_BATCH) {
                let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let exact_sql =
                    format!("SELECT id, embedding FROM facts WHERE id IN ({placeholders})");
                let mut stmt = conn.prepare(&exact_sql)?;
                let binds: Vec<Value> = chunk.iter().map(|i| Value::Integer(*i)).collect();
                let rows = stmt.query_map(params_from_iter(binds), |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
                })?;
                for row in rows {
                    let (id, blob) = row?;
                    let v = from_bytes(&blob);
                    let sim = cosine(&q_emb, &v);
                    sims.push((id, sim, v));
                }
            }
        }
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        // MMR (λ=0.7) keeps relevance dominant while breaking up near-duplicates.
        let semantic: Vec<i64> = crate::rerank::mmr(&sims, MMR_LAMBDA, ARM_LIMIT);

        // --- Reciprocal Rank Fusion ---
        let mut score: HashMap<i64, f32> = HashMap::new();
        let mut kw_rank: HashMap<i64, usize> = HashMap::new();
        let mut sem_rank: HashMap<i64, usize> = HashMap::new();
        for (i, id) in keyword.iter().enumerate() {
            *score.entry(*id).or_default() += 1.0 / (RRF_K + (i as f32) + 1.0);
            kw_rank.insert(*id, i + 1);
        }
        for (i, id) in semantic.iter().enumerate() {
            *score.entry(*id).or_default() += 1.0 / (RRF_K + (i as f32) + 1.0);
            sem_rank.insert(*id, i + 1);
        }
        // Specificity boost: when a hit lives in the active project or session ring, nudge it above
        // an equally-scored user-global hit, so the more-specific memory wins ties without burying a
        // strongly-relevant global fact. Only meaningful when a specific ring is active; the whole-
        // brain and user-only views leave RRF ordering untouched (preserving legacy behaviour).
        if scope.project.is_some() || scope.session.is_some() {
            apply_scope_boost(&conn, &mut score)?;
        }
        // Cold-tier deprioritization: consolidate() demotes stale, low-importance rows to 'cold' as
        // its "sleep cycle" step, but until this nothing ever read `tier` at recall time - a demoted
        // row ranked identically to a warm one, making the demotion pure bookkeeping with no
        // observable effect (the decorative-theater failure mode). A small penalty, not a hard
        // exclusion, so a cold memory is still fully recallable when it's genuinely the best match.
        apply_tier_penalty(&conn, &mut score)?;
        let mut fused: Vec<(i64, f32)> = score.into_iter().collect();
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        fused.truncate(k);

        // --- materialise records, in fused order, and record access ---
        let now = now_ms() as i64;
        let mut hits = Vec::with_capacity(fused.len());
        for (id, sc) in fused {
            if let Some(record) = get_record(&conn, id)? {
                conn.execute(
                    "UPDATE facts SET last_access_ms = ?1, access_count = access_count + 1 WHERE id = ?2",
                    params![now, id],
                )?;
                // The data mutation above happens on every hit (it's what makes recency real-time
                // for scoring/consolidation); the ledger record of it is batched (see
                // `record_access`) so a hot recall path doesn't append dozens of near-noise entries.
                self.record_access(id);
                hits.push(Hit {
                    record,
                    score: sc,
                    keyword_rank: kw_rank.get(&id).copied(),
                    semantic_rank: sem_rank.get(&id).copied(),
                });
            }
        }
        Ok(hits)
    }

    /// Tombstone a memory (excluded from recall) and record it. Reversible via [`restore`].
    pub fn forget(&self, id: i64, actor: &str, reason: &str) -> Result<bool> {
        let content_hash: Option<String> = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.query_row(
                "SELECT content_hash FROM facts WHERE id = ?1 AND deleted = 0",
                [id],
                |r| r.get(0),
            )
            .optional()?
        };
        let Some(content_hash) = content_hash else {
            return Ok(false);
        };
        let entry = self.ledger.append(
            "memory.forget",
            actor,
            json!({ "id": id, "content_hash": content_hash, "reason": reason }),
        )?;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        conn.execute(
            "UPDATE facts SET deleted = 1, ledger_seq = ?1 WHERE id = ?2",
            params![entry.seq as i64, id],
        )?;
        Ok(true)
    }

    /// Tombstone every live memory in a non-user scope (a project or session) — the cascade for
    /// deleting that project/session, so its facts don't linger to bleed into other recalls (the
    /// exact cross-project-bleed failure the scope lattice exists to prevent). Refuses the
    /// user-global ring and an empty scope id (either would erase durable facts about the person).
    /// Ledger-first: one signed `memory.forget_scope` entry records the sweep. Returns the count.
    pub fn forget_scope(
        &self,
        scope_kind: &str,
        scope_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<usize> {
        if scope_kind == "user" || scope_kind.is_empty() || scope_id.is_empty() {
            return Ok(0);
        }
        let count: i64 = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.query_row(
                "SELECT COUNT(*) FROM facts WHERE scope_kind = ?1 AND scope_id = ?2 AND deleted = 0",
                params![scope_kind, scope_id],
                |r| r.get(0),
            )?
        };
        if count == 0 {
            return Ok(0);
        }
        // Ledger-first (matches forget/supersede's I1 ordering): record the sweep before mutating.
        let entry = self.ledger.append(
            "memory.forget_scope",
            actor,
            json!({ "scope_kind": scope_kind, "scope_id": scope_id, "count": count, "reason": reason }),
        )?;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        conn.execute(
            "UPDATE facts SET deleted = 1, ledger_seq = ?1 WHERE scope_kind = ?2 AND scope_id = ?3 AND deleted = 0",
            params![entry.seq as i64, scope_kind, scope_id],
        )?;
        Ok(count as usize)
    }

    /// Mark `old_id` as superseded by `by_id`: the old fact becomes history - kept (not
    /// deleted) and still in the ledger, but no longer surfaced by recall - while the new
    /// fact is the current truth. This is how a changed fact ("moved to Munich") evolves
    /// without erasing the past. Returns false if `old_id` wasn't a current, live memory.
    pub fn supersede(&self, old_id: i64, by_id: i64) -> Result<bool> {
        // Ledger-first (I1), matching forget/remember: confirm the row is a current, live memory,
        // then append the signed entry, then apply the mutation. Superseding is what recall SHOWS, so
        // an unaudited supersede would be exactly the kind of silent memory rewrite the signed ledger
        // is meant to make impossible - hence the append error propagates rather than being dropped.
        let supersedable: bool = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.query_row(
                "SELECT 1 FROM facts WHERE id = ?1 AND deleted = 0 AND superseded_by IS NULL",
                [old_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        };
        if !supersedable {
            return Ok(false);
        }
        let now = now_ms() as i64;
        let entry = self.ledger.append(
            "memory.supersede",
            "core",
            json!({ "old": old_id, "by": by_id, "valid_until_ms": now }),
        )?;
        let n = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            // Stamps valid_until_ms on the OLD row alongside superseded_by (bi-temporal versioning,
            // additive - superseded_by stays the UI-facing "current truth" pointer every existing
            // call site already filters on; valid_until_ms only feeds recall_as_of). Both are set in
            // the SAME statement so they can never drift apart.
            conn.execute(
                "UPDATE facts SET superseded_by = ?1, ledger_seq = ?2, valid_until_ms = ?3 \
                 WHERE id = ?4 AND deleted = 0 AND superseded_by IS NULL",
                params![by_id, entry.seq as i64, now, old_id],
            )?
        };
        Ok(n > 0)
    }

    /// Find live, current (non-deleted, non-superseded) facts in `region`+`scope` whose meaning is
    /// close to `text` but whose content is NOT identical (exact duplicates are `remember()`'s
    /// dedup-on-write's job, not this one) - the candidate set a contradiction-detector should ask
    /// a model to judge. Reuses the same exact-cosine machinery `recall_inner`'s semantic arm uses;
    /// `min_similarity` is the caller's threshold (this function does no ranking-quality judgment
    /// of its own beyond cosine distance). Bounded to a small `limit` (a candidate LISTING a model
    /// reads, not a recall result set).
    pub fn find_similar_not_identical(
        &self,
        region: Region,
        scope: &Scope,
        text: &str,
        min_similarity: f32,
        limit: usize,
    ) -> Result<Vec<Record>> {
        let q_emb = self.embedder.embed(text);
        let content_hash = blake3::hash(text.as_bytes()).to_hex().to_string();
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM facts WHERE region = ?1 AND scope_kind = ?2 AND scope_id = ?3 \
             AND deleted = 0 AND superseded_by IS NULL AND content_hash != ?4"
        ))?;
        let rows = stmt.query_map(
            params![region.as_str(), scope.kind.as_str(), scope.id, content_hash],
            map_record,
        )?;
        let mut scored: Vec<(f32, Record)> = Vec::new();
        for row in rows {
            let rec = row?;
            let emb = {
                let blob: Vec<u8> = conn.query_row(
                    "SELECT embedding FROM facts WHERE id = ?1",
                    [rec.id],
                    |r| r.get(0),
                )?;
                from_bytes(&blob)
            };
            let sim = cosine(&q_emb, &emb);
            if sim >= min_similarity {
                scored.push((sim, rec));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }

    /// Record a detected-but-unconfirmed contradiction. Never applies it - the caller (a
    /// contradiction-detector) has already run its own citation-verified judgment; this only
    /// persists the proposal for a human to accept or reject via [`Memory::resolve_supersession`].
    /// Ledger-first, like every other mutation.
    pub fn propose_supersession(
        &self,
        old_id: i64,
        candidate_text: &str,
        reason: &str,
        region: Region,
        scope: &Scope,
        actor: &str,
    ) -> Result<i64> {
        let now = now_ms() as i64;
        let entry = self.ledger.append(
            "memory.propose_supersession",
            actor,
            json!({ "old_id": old_id, "candidate_text": candidate_text, "reason": reason }),
        )?;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        conn.execute(
            "INSERT INTO pending_supersessions \
             (old_id, candidate_text, reason, region, scope_kind, scope_id, actor, created_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                old_id,
                candidate_text,
                reason,
                region.as_str(),
                scope.kind.as_str(),
                scope.id,
                actor,
                now,
            ],
        )?;
        let _ = entry;
        Ok(conn.last_insert_rowid())
    }

    /// All not-yet-resolved proposed contradictions, oldest first.
    pub fn pending_supersessions(&self) -> Result<Vec<PendingSupersession>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, old_id, candidate_text, reason, region, scope_kind, scope_id, actor, created_ms \
             FROM pending_supersessions WHERE resolved = 0 ORDER BY created_ms ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PendingSupersession {
                id: r.get(0)?,
                old_id: r.get(1)?,
                candidate_text: r.get(2)?,
                reason: r.get(3)?,
                region: r.get(4)?,
                scope_kind: r.get(5)?,
                scope_id: r.get(6)?,
                actor: r.get(7)?,
                created_ms: r.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Resolve a pending contradiction: `accept` writes the candidate as a new fact and supersedes
    /// the old one (through the existing, ledgered `remember`/`supersede` - no separate mutation
    /// path); rejecting just marks it resolved, writing and superseding nothing. Either way this is
    /// the ONLY place a proposed contradiction can ever take effect - never automatically.
    pub fn resolve_supersession(&self, id: i64, accept: bool, actor: &str) -> Result<bool> {
        let row: Option<(i64, String, String, String, String, String)> = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.query_row(
                "SELECT old_id, candidate_text, region, scope_kind, scope_id, reason \
                 FROM pending_supersessions WHERE id = ?1 AND resolved = 0",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?
        };
        let Some((old_id, candidate_text, region, scope_kind, scope_id, reason)) = row else {
            return Ok(false);
        };
        let new_id = if accept {
            let region = match region.as_str() {
                "episodic" => Region::Episodic,
                "identity" => Region::Identity,
                "procedural" => Region::Procedural,
                _ => Region::Semantic,
            };
            let scope = Scope::from_parts(&scope_kind, &scope_id);
            let rec = self.remember(
                WriteReq::new(region, candidate_text)
                    .source("contradiction_detector")
                    .actor(actor)
                    .scope(scope),
            )?;
            self.supersede(old_id, rec.id)?;
            Some(rec.id)
        } else {
            None
        };
        let now = now_ms() as i64;
        self.ledger.append(
            "memory.resolve_supersession",
            actor,
            json!({ "id": id, "accepted": accept, "old_id": old_id, "new_id": new_id, "reason": reason }),
        )?;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        conn.execute(
            "UPDATE pending_supersessions SET resolved = ?1, resolved_ms = ?2, new_id = ?3 WHERE id = ?4",
            params![if accept { 1 } else { 2 }, now, new_id, id],
        )?;
        Ok(true)
    }

    /// IDs of current (live, non-superseded) memories in `region` whose text starts with
    /// `prefix`, across ALL scopes - used to find the prior singular fact a new one replaces.
    pub fn current_with_prefix(&self, region: Region, prefix: &str) -> Result<Vec<i64>> {
        self.current_with_prefix_scoped(region, prefix, &ScopeCtx::any())
    }

    /// Like [`current_with_prefix`], but restricted to the rings in `scope`, so a superseding
    /// write only replaces a prior fact IN THE SAME RING (a global name-change never reaches into
    /// a project ring, and vice-versa).
    pub fn current_with_prefix_scoped(
        &self,
        region: Region,
        prefix: &str,
        scope: &ScopeCtx,
    ) -> Result<Vec<i64>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let (scope_sql, scope_binds) = scope_clause("", scope);
        let sql = format!(
            "SELECT id FROM facts \
             WHERE region = ? AND deleted = 0 AND superseded_by IS NULL AND text LIKE ? AND {scope}",
            scope = scope_sql,
        );
        let like = format!("{prefix}%"); // prefixes are fixed RULE strings, no LIKE wildcards
        let mut binds: Vec<Value> = Vec::with_capacity(scope_binds.len() + 2);
        binds.push(Value::Text(region.as_str().to_string()));
        binds.push(Value::Text(like));
        for id in scope_binds {
            binds.push(Value::Text(id));
        }
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(binds), |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Every live memory with EXACTLY this `source` value, oldest first - how a paged-out
    /// compaction exhaust (`source = "compaction:<run_tag>"`) or any other exact-source-tagged
    /// write group gets found again later. Scoped like every other model-facing read, so a page-in
    /// tool can't reach across rings any more than ordinary recall can.
    pub fn by_source_scoped(&self, source: &str, scope: &ScopeCtx) -> Result<Vec<Record>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let (scope_sql, scope_binds) = scope_clause("", scope);
        let sql = format!(
            "SELECT {COLS} FROM facts WHERE source = ? AND deleted = 0 AND {scope} ORDER BY id ASC",
            scope = scope_sql,
        );
        let mut binds: Vec<Value> = Vec::with_capacity(scope_binds.len() + 1);
        binds.push(Value::Text(source.to_string()));
        for id in scope_binds {
            binds.push(Value::Text(id));
        }
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(binds), map_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Promote a memory to the user-global ring - e.g. a project fact that turns out to be a durable
    /// cross-project preference, so it should follow the user everywhere. Ledgered and reversible.
    /// Refuses to promote UNTRUSTED-provenance memory into the trusted user-global ring (the
    /// injection guard: scraped/attacker content must never become a global fact about the user).
    /// Returns `Ok(false)` if the id isn't a live memory.
    pub fn promote_to_user(&self, id: i64, actor: &str) -> Result<bool> {
        let row: Option<(String, String)> = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.query_row(
                "SELECT taint, scope_kind FROM facts WHERE id = ?1 AND deleted = 0",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        let Some((taint, scope_kind)) = row else {
            return Ok(false);
        };
        if taint != "trusted" {
            return Err(MemoryError::UntrustedPromotion);
        }
        if scope_kind == "user" {
            return Ok(true); // already global, nothing to do
        }
        let entry =
            self.ledger
                .append("memory.promote", actor, json!({ "id": id, "to": "user" }))?;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        conn.execute(
            "UPDATE facts SET scope_kind = 'user', scope_id = '', ledger_seq = ?1 WHERE id = ?2",
            params![entry.seq as i64, id],
        )?;
        Ok(true)
    }

    /// Undo a [`forget`], bringing the memory back into recall.
    pub fn restore(&self, id: i64, actor: &str) -> Result<bool> {
        let exists: Option<i64> = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.query_row(
                "SELECT id FROM facts WHERE id = ?1 AND deleted = 1",
                [id],
                |r| r.get(0),
            )
            .optional()?
        };
        if exists.is_none() {
            return Ok(false);
        }
        let entry = self
            .ledger
            .append("memory.restore", actor, json!({ "id": id }))?;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        conn.execute(
            "UPDATE facts SET deleted = 0, ledger_seq = ?1 WHERE id = ?2",
            params![entry.seq as i64, id],
        )?;
        Ok(true)
    }

    /// Fetch a single live memory by id.
    pub fn get(&self, id: i64) -> Result<Option<Record>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        get_record(&conn, id)
    }

    /// The most recent `n` memories in a region across ALL scopes, oldest-first - used to reload a
    /// conversation from episodic memory so the chat survives a refresh.
    pub fn recent(&self, region: Region, n: usize) -> Result<Vec<Record>> {
        self.recent_scoped(region, n, &ScopeCtx::any())
    }

    /// Like [`recent`], but restricted to the rings in `scope`. Consciousness distillation uses
    /// this to build a user-global working-memory block and a separate per-project block.
    pub fn recent_scoped(&self, region: Region, n: usize, scope: &ScopeCtx) -> Result<Vec<Record>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let (scope_sql, scope_binds) = scope_clause("", scope);
        // `superseded_by IS NULL` keeps this to CURRENT facts only (mirrors recent_in_ring). Without
        // it, a superseded row ("User lives Berlin") stays a distillation candidate alongside the
        // fact that replaced it ("I live in Munich"), and consciousness would load the stale one into
        // the AUTHORITATIVE working-memory block - the exact supersede-defeating bug this guards.
        let sql = format!(
            "SELECT {COLS} FROM facts WHERE region = ? AND deleted = 0 AND superseded_by IS NULL \
             AND {scope} ORDER BY created_ms DESC LIMIT ?",
            scope = scope_sql,
        );
        let mut binds: Vec<Value> = Vec::with_capacity(scope_binds.len() + 2);
        binds.push(Value::Text(region.as_str().to_string()));
        for id in scope_binds {
            binds.push(Value::Text(id));
        }
        binds.push(Value::Integer(n as i64));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(binds), map_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out.reverse();
        Ok(out)
    }

    /// The most recent `n` memories in a region within a SINGLE ring (exact scope), most-recent
    /// first - used to build a per-project working-memory block that must contain *only* that
    /// project's facts (not the user-global ones, which the global block already carries).
    pub fn recent_in_ring(&self, region: Region, n: usize, scope: &Scope) -> Result<Vec<Record>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let sql = format!(
            "SELECT {COLS} FROM facts WHERE region = ? AND deleted = 0 AND superseded_by IS NULL \
             AND scope_kind = ? AND scope_id = ? ORDER BY created_ms DESC LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![region.as_str(), scope.kind.as_str(), scope.id, n as i64],
            map_record,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Consolidate: demote warm memories that are stale *and* low-importance to the
    /// cold tier - the machine analogue of sleep moving the day's noise out of the
    /// way while keeping what mattered close.
    pub fn consolidate(&self, warm_age: Duration) -> Result<i64> {
        let now = now_ms() as i64;
        let cutoff = warm_age.as_millis() as i64;
        // Ledger-first (I1): count the rows this consolidation WILL demote, append the signed entry,
        // then apply the demotion - all under one held lock, so no concurrent write can slip in
        // between the count and the UPDATE (append never re-enters the connection, so holding the
        // lock across it is safe). Propagate the append error rather than demoting unaudited.
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let predicate =
            "tier = 'warm' AND deleted = 0 AND (?1 - last_access_ms) > ?2 AND importance < 0.7";
        let would_demote: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM facts WHERE {predicate}"),
            params![now, cutoff],
            |r| r.get(0),
        )?;
        if would_demote == 0 {
            return Ok(0);
        }
        self.ledger.append(
            "memory.consolidate",
            "core",
            json!({ "demoted": would_demote }),
        )?;
        let demoted = conn.execute(
            &format!("UPDATE facts SET tier = 'cold' WHERE {predicate}"),
            params![now, cutoff],
        )? as i64;
        Ok(demoted)
    }

    /// Find small, bounded, co-scoped groups of related Trusted-only facts worth reflecting on -
    /// the candidate-selection half of Phase D's grounded reflection pass
    /// (docs/MEMORY-UPGRADE-PLAN.md §6 Phase D). Deliberately NOT RAPTOR-style clustering (locked
    /// decision, §5): it scans the exact same warm/stale/low-importance candidate set
    /// [`Memory::consolidate`] already fetches - restricted to Trusted provenance (a reflection
    /// must never fire on tainted content) - and does a simple greedy pairwise-cosine grouping
    /// within each (region, scope) ring, so a synthesis never mixes facts from different worlds.
    /// Groups are capped at `max_groups` (a hard bound on how many single-LLM-call syntheses one
    /// tick can trigger); a cluster smaller than 2 members is dropped (nothing to synthesize from
    /// one fact alone). Ring iteration order is sorted for determinism (tests, and so a bounded
    /// `max_groups` truncation is reproducible, not an arbitrary hash-order sample).
    pub fn reflection_candidates(
        &self,
        warm_age: Duration,
        min_similarity: f32,
        max_groups: usize,
    ) -> Result<Vec<Vec<Record>>> {
        let now = now_ms() as i64;
        let cutoff = warm_age.as_millis() as i64;
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let candidates: Vec<Record> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLS} FROM facts \
                 WHERE tier = 'warm' AND deleted = 0 AND superseded_by IS NULL AND taint = 'trusted' \
                 AND (?1 - last_access_ms) > ?2 AND importance < 0.7 \
                 ORDER BY region, scope_kind, scope_id, id LIMIT ?3"
            ))?;
            let rows = stmt.query_map(
                params![now, cutoff, REFLECTION_CANDIDATE_LIMIT as i64],
                map_record,
            )?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };
        // Fetch each candidate's embedding alongside it (COLS omits it - an internal index, not
        // content).
        let mut embeddings: HashMap<i64, Vec<f32>> = HashMap::with_capacity(candidates.len());
        for rec in &candidates {
            let blob: Vec<u8> =
                conn.query_row("SELECT embedding FROM facts WHERE id = ?1", [rec.id], |r| {
                    r.get(0)
                })?;
            embeddings.insert(rec.id, from_bytes(&blob));
        }
        drop(conn);

        // Group by (region, scope) so a synthesis never mixes facts from different worlds.
        let mut by_ring: HashMap<(String, String, String), Vec<Record>> = HashMap::new();
        for rec in candidates {
            by_ring
                .entry((rec.region.clone(), rec.scope_kind.clone(), rec.scope_id.clone()))
                .or_default()
                .push(rec);
        }
        let mut rings: Vec<_> = by_ring.into_iter().collect();
        rings.sort_by(|a, b| a.0.cmp(&b.0));

        let mut groups: Vec<Vec<Record>> = Vec::new();
        for (_, ring) in rings {
            let mut used = vec![false; ring.len()];
            for i in 0..ring.len() {
                if used[i] {
                    continue;
                }
                let mut cluster = vec![i];
                used[i] = true;
                for (j, u) in used.iter_mut().enumerate().skip(i + 1) {
                    if *u {
                        continue;
                    }
                    let sim = cosine(&embeddings[&ring[i].id], &embeddings[&ring[j].id]);
                    if sim >= min_similarity {
                        cluster.push(j);
                        *u = true;
                    }
                }
                if cluster.len() >= 2 {
                    groups.push(cluster.into_iter().map(|k| ring[k].clone()).collect());
                }
            }
        }
        groups.truncate(max_groups);
        Ok(groups)
    }

    /// The third of the sleep-cycle triad (move/strengthen/prune) - conservative, reversible,
    /// automatic forgetting. Prunes only rows that are ALREADY superseded (a newer fact replaced
    /// them, so `superseded_by IS NOT NULL` already makes them invisible to every recall path
    /// unconditionally - pruning removes pure clutter, not information anyone could still see) and
    /// old enough (`min_age`) that nobody would plausibly need them for a not-yet-built time-travel
    /// view. Calls the existing `forget()` (ledgered, restorable) rather than a new deletion path.
    /// Opt-in by design (see `security.auto_prune_memories` in engramd) - intentionally NOT invoked
    /// unless the caller enables it, matching the `auto_distill_skills` pattern for automatic,
    /// content-destructive-looking-but-reversible actions.
    pub fn auto_prune(&self, min_age: Duration, actor: &str) -> Result<usize> {
        let now = now_ms() as i64;
        let cutoff = min_age.as_millis() as i64;
        let ids: Vec<i64> = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            let mut stmt = conn.prepare(
                "SELECT id FROM facts \
                 WHERE deleted = 0 AND superseded_by IS NOT NULL AND (?1 - created_ms) > ?2",
            )?;
            let rows = stmt.query_map(params![now, cutoff], |r| r.get::<_, i64>(0))?;
            let mut v = Vec::new();
            for row in rows {
                v.push(row?);
            }
            v
        };
        let mut pruned = 0usize;
        for id in ids {
            if self.forget(id, actor, "auto_prune: superseded and past the retention window")? {
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// Rebuild the derived binary coarse index (`embedding_bin`) for EVERY row from its stored
    /// embedding. The binary codes are derived state, not the source of truth, so this fully repairs
    /// a corrupt or partially-populated index without touching content or the ledger. Returns the
    /// number of rows reindexed. (`backfill_binary` only fills NULLs; this recomputes all.)
    pub fn reindex_binary(&self) -> Result<i64> {
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        let rows: Vec<(i64, Vec<u8>)> = {
            let mut stmt = conn.prepare("SELECT id, embedding FROM facts")?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
            let mut v = Vec::new();
            for row in mapped {
                v.push(row?);
            }
            v
        };
        let n = rows.len() as i64;
        let tx = conn.transaction()?;
        for (id, blob) in &rows {
            let bin = quantize_binary(&from_bytes(blob));
            tx.execute(
                "UPDATE facts SET embedding_bin = ?1 WHERE id = ?2",
                params![bin, id],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Counts for the Memory Atlas.
    pub fn stats(&self) -> Result<Stats> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let total = conn.query_row("SELECT COUNT(*) FROM facts WHERE deleted = 0", [], |r| {
            r.get(0)
        })?;
        let by_region = group_count(&conn, "region")?;
        let by_tier = group_count(&conn, "tier")?;
        let by_actor = group_count(&conn, "actor")?;
        let needs_reembed = conn.query_row(
            "SELECT COUNT(*) FROM facts WHERE deleted = 0 AND needs_reembed = 1",
            [],
            |r| r.get(0),
        )?;
        Ok(Stats {
            total,
            by_region,
            by_tier,
            by_actor,
            needs_reembed,
        })
    }

    /// The same breakdown as [`Memory::stats`], scoped to ONE ring (a specific project or session,
    /// or the user-global ring) - so "this project has N memories, 12% procedural, 3 from agent
    /// Atlas" becomes queryable, which nothing in the stats surface could answer before (`stats()`
    /// is global-only). Pass `("user", "")` for the user-global ring.
    pub fn stats_for_scope(&self, scope_kind: &str, scope_id: &str) -> Result<Stats> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let total = conn.query_row(
            "SELECT COUNT(*) FROM facts WHERE deleted = 0 AND scope_kind = ?1 AND scope_id = ?2",
            params![scope_kind, scope_id],
            |r| r.get(0),
        )?;
        let by_region = group_count_scoped(&conn, "region", scope_kind, scope_id)?;
        let by_tier = group_count_scoped(&conn, "tier", scope_kind, scope_id)?;
        let by_actor = group_count_scoped(&conn, "actor", scope_kind, scope_id)?;
        let needs_reembed = conn.query_row(
            "SELECT COUNT(*) FROM facts WHERE deleted = 0 AND needs_reembed = 1 AND scope_kind = ?1 AND scope_id = ?2",
            params![scope_kind, scope_id],
            |r| r.get(0),
        )?;
        Ok(Stats {
            total,
            by_region,
            by_tier,
            by_actor,
            needs_reembed,
        })
    }

    /// The active embedder's identity string (e.g. `trigram-hash-v1`, `static-model2vec-v1`, or
    /// `gateway:<provider>:<model>`) - what actually embedded the vectors on disk right now,
    /// regardless of what was configured, so a caller can tell the two apart.
    pub fn embedder_name(&self) -> &str {
        self.embedder.name()
    }

    /// Re-embed every row flagged `needs_reembed` (rows written while the configured embedder was
    /// degraded, e.g. a gateway outage that fell back to trigram) using the CURRENT embedder.
    /// A no-op when nothing is flagged. Intended to run periodically (the hourly consolidation
    /// tick) once the real embedder is healthy again, closing the gap where a degraded write used
    /// to persist a mis-ranked vector with no repair path. Ledger-first, like every other mutation.
    pub fn reembed_flagged(&self) -> Result<usize> {
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        let rows: Vec<(i64, String)> = {
            let mut stmt =
                conn.prepare("SELECT id, text FROM facts WHERE needs_reembed = 1 AND deleted = 0")?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            let mut v = Vec::new();
            for row in mapped {
                v.push(row?);
            }
            v
        };
        if rows.is_empty() {
            return Ok(0);
        }
        self.ledger.append(
            "memory.reembed_flagged",
            "core",
            json!({ "rows": rows.len(), "embedder": self.embedder.name() }),
        )?;
        let tx = conn.transaction()?;
        let mut fixed = 0usize;
        for (id, text) in &rows {
            let (v, degraded) = self.embedder.embed_checked(text);
            // Still degraded (embedder still unhealthy): leave the flag set, try again next pass.
            if degraded {
                continue;
            }
            tx.execute(
                "UPDATE facts SET embedding = ?1, embedding_bin = ?2, needs_reembed = 0 WHERE id = ?3",
                params![to_bytes(&v), quantize_binary(&v), id],
            )?;
            fixed += 1;
        }
        tx.commit()?;
        Ok(fixed)
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS facts (
            id            INTEGER PRIMARY KEY,
            region        TEXT NOT NULL,
            text          TEXT NOT NULL,
            importance    REAL NOT NULL DEFAULT 0.5,
            taint         TEXT NOT NULL DEFAULT 'trusted',
            tier          TEXT NOT NULL DEFAULT 'warm',
            source        TEXT,
            metadata      TEXT,
            embedding     BLOB NOT NULL,
            content_hash  TEXT NOT NULL,
            ledger_seq    INTEGER,
            created_ms    INTEGER NOT NULL,
            last_access_ms INTEGER NOT NULL,
            access_count  INTEGER NOT NULL DEFAULT 0,
            deleted       INTEGER NOT NULL DEFAULT 0,
            superseded_by INTEGER,
            scope_kind    TEXT NOT NULL DEFAULT 'user',
            scope_id      TEXT NOT NULL DEFAULT '',
            embedding_bin BLOB
        );
        CREATE INDEX IF NOT EXISTS idx_facts_region ON facts(region, deleted);
        CREATE INDEX IF NOT EXISTS idx_facts_salience ON facts(importance DESC, last_access_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_facts_dedup ON facts(region, content_hash);
        CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(text, tokenize = 'unicode61');
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE IF NOT EXISTS pending_supersessions (
            id             INTEGER PRIMARY KEY,
            old_id         INTEGER NOT NULL,
            candidate_text TEXT NOT NULL,
            reason         TEXT NOT NULL,
            region         TEXT NOT NULL,
            scope_kind     TEXT NOT NULL,
            scope_id       TEXT NOT NULL,
            actor          TEXT NOT NULL,
            created_ms     INTEGER NOT NULL,
            resolved       INTEGER NOT NULL DEFAULT 0,
            resolved_ms    INTEGER,
            new_id         INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_pending_supersessions_resolved ON pending_supersessions(resolved);",
    )?;
    // Idempotent column adds for brains created before a feature existed (each is a no-op,
    // and errors "duplicate column", when the column is already present - hence `let _`).
    // The supersession column predates temporal validity; the scope columns partition memory
    // into user/project/session rings (default 'user' makes every legacy row user-global, so
    // nothing disappears and the bleed only stops accruing forward).
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN superseded_by INTEGER", []);
    let _ = conn.execute(
        "ALTER TABLE facts ADD COLUMN scope_kind TEXT NOT NULL DEFAULT 'user'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE facts ADD COLUMN scope_id TEXT NOT NULL DEFAULT ''",
        [],
    );
    // The binary-quantized companion of `embedding` - the coarse index for two-stage recall.
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN embedding_bin BLOB", []);
    // Set when `remember()` had to fall back to a degraded embedding (e.g. the gateway model was
    // unreachable), so a background pass can find and re-embed exactly these rows once the real
    // embedder is healthy again, instead of leaving them silently mis-ranked forever.
    let _ = conn.execute(
        "ALTER TABLE facts ADD COLUMN needs_reembed INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Who wrote this fact - `user`, `core`, a skill id, or `agent:<name>` for a durable named
    // agent's own writes. Attribution, not a new scope ring: an agent's memory stays in the
    // ordinary project/user ring (so the rest of the team still sees it), this just makes "which
    // agent said this" a queryable column instead of a ledger cross-reference.
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN actor TEXT NOT NULL DEFAULT ''", []);
    // Bi-temporal fact versioning, additive alongside the existing `superseded_by` "current truth"
    // pointer (unchanged, still what every default recall filters on): `valid_from_ms` is when a
    // fact became true, `valid_until_ms` (NULL = still current) is when `supersede()` replaced it.
    // Only `Memory::recall_as_of` ever reads these - the default recall path is untouched.
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN valid_from_ms INTEGER", []);
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN valid_until_ms INTEGER", []);
    // RAPTOR-style tree-summary groundwork (docs/MEMORY-UPGRADE-PLAN.md §4 / Phase D): `tree_level`
    // is 0 for every ordinary fact, 1 for a reflection-pass synthesis (see `Record::tree_level`).
    // `parent_id` has no writer yet - reserved so a future second-level reflection pass (a synthesis
    // OF syntheses) doesn't need a second schema migration to point a fact at its own summary.
    let _ = conn.execute(
        "ALTER TABLE facts ADD COLUMN tree_level INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN parent_id INTEGER", []);
    // Backfill: every row's own creation time is a safe, always-correct valid_from default.
    let _ = conn.execute(
        "UPDATE facts SET valid_from_ms = created_ms WHERE valid_from_ms IS NULL",
        [],
    );
    // Backfill for rows already superseded before this migration existed: the moment its
    // replacement was created is an accurate (often exact) stand-in for when it stopped being
    // valid, since supersede() is always called right after the replacement is written.
    let _ = conn.execute(
        "UPDATE facts SET valid_until_ms = (SELECT created_ms FROM facts b WHERE b.id = facts.superseded_by) \
         WHERE superseded_by IS NOT NULL AND valid_until_ms IS NULL",
        [],
    );
    // Created AFTER the ALTERs so the referenced columns exist on a migrated legacy brain too.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_facts_scope ON facts(scope_kind, scope_id, deleted)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_facts_actor ON facts(actor, deleted)",
        [],
    )?;
    Ok(())
}

/// Count live rows grouped by a fixed column name (`region` or `tier`).
fn group_count(conn: &Connection, column: &str) -> Result<HashMap<String, i64>> {
    let sql = format!("SELECT {column}, COUNT(*) FROM facts WHERE deleted = 0 GROUP BY {column}");
    let mut stmt = conn.prepare(&sql)?;
    let mut out = HashMap::new();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (k, v) = row?;
        out.insert(k, v);
    }
    Ok(out)
}

/// Like [`group_count`], restricted to one scope ring. `column` is always a fixed internal name
/// (`"region"`/`"tier"`/`"actor"`), never user input, so interpolating it is injection-free.
fn group_count_scoped(
    conn: &Connection,
    column: &str,
    scope_kind: &str,
    scope_id: &str,
) -> Result<HashMap<String, i64>> {
    let sql = format!(
        "SELECT {column}, COUNT(*) FROM facts WHERE deleted = 0 AND scope_kind = ?1 AND scope_id = ?2 GROUP BY {column}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut out = HashMap::new();
    for row in stmt.query_map(params![scope_kind, scope_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (k, v) = row?;
        out.insert(k, v);
    }
    Ok(out)
}

fn get_record(conn: &Connection, id: i64) -> Result<Option<Record>> {
    let sql = format!("SELECT {COLS} FROM facts WHERE id = ?1 AND deleted = 0");
    Ok(conn.query_row(&sql, [id], map_record).optional()?)
}

fn map_record(r: &Row) -> rusqlite::Result<Record> {
    let meta: Option<String> = r.get(7)?;
    Ok(Record {
        id: r.get(0)?,
        region: r.get(1)?,
        text: r.get(2)?,
        importance: r.get::<_, f64>(3)? as f32,
        taint: r.get(4)?,
        tier: r.get(5)?,
        source: r.get(6)?,
        metadata: meta
            .and_then(|m| serde_json::from_str(&m).ok())
            .unwrap_or(serde_json::Value::Null),
        content_hash: r.get(8)?,
        ledger_seq: r.get(9)?,
        created_ms: r.get(10)?,
        last_access_ms: r.get(11)?,
        access_count: r.get(12)?,
        scope_kind: r.get(13)?,
        scope_id: r.get(14)?,
        actor: r.get(15)?,
        valid_from_ms: r.get(16)?,
        valid_until_ms: r.get(17)?,
        tree_level: r.get(18)?,
    })
}

/// Build an FTS5 MATCH query: each ≥2-char token quoted and OR-ed, so punctuation
/// can't break the query and any term can match.
fn build_match(query: &str) -> Option<String> {
    let toks: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| format!("\"{t}\""))
        .collect();
    if toks.is_empty() {
        None
    } else {
        Some(toks.join(" OR "))
    }
}

/// A safe `region IN (...)` clause. Region strings come from a fixed enum, never user
/// input, so inlining them is injection-free and avoids dynamic placeholder juggling.
fn region_clause(prefix: &str, regions: &[Region]) -> String {
    if regions.is_empty() {
        return "1 = 1".into();
    }
    let list = regions
        .iter()
        .map(|r| format!("'{}'", r.as_str()))
        .collect::<Vec<_>>()
        .join(",");
    format!("{prefix}region IN ({list})")
}

/// The union-of-rings scope predicate and its bound `scope_id` values. A whole-brain context
/// (`ScopeCtx::any()`) yields `("1 = 1", [])` - no ring filter. `scope_kind` is a fixed enum
/// string (safe to inline); `scope_id` is an opaque workspace id, so it is always bound, never
/// interpolated. `prefix` qualifies the columns for a joined query (e.g. `"f."`).
fn scope_clause(prefix: &str, scope: &ScopeCtx) -> (String, Vec<String>) {
    let rings = scope.rings();
    if rings.is_empty() {
        return ("1 = 1".into(), Vec::new());
    }
    let mut parts = Vec::with_capacity(rings.len());
    let mut binds = Vec::with_capacity(rings.len());
    for (kind, id) in rings {
        parts.push(format!(
            "({prefix}scope_kind = '{}' AND {prefix}scope_id = ?)",
            kind.as_str()
        ));
        binds.push(id);
    }
    (format!("({})", parts.join(" OR ")), binds)
}

/// Session > project > user tie-break, sized at roughly one RRF rank-step so it settles ties
/// without overpowering genuine relevance. Applied only to the bounded fused candidate set.
const SCOPE_BOOST_PROJECT: f32 = 0.010;
const SCOPE_BOOST_SESSION: f32 = 0.018;
/// A cold-tier memory is deprioritized, not excluded - smaller than either scope boost, so a
/// genuinely relevant cold memory still outranks an unrelated warm one; it only settles a
/// near-tie in the warm row's favor.
const TIER_PENALTY_COLD: f32 = 0.005;

/// Add a small specificity boost to project/session-ringed hits among the fused candidates. Looks
/// up the scope of only those candidate ids (a bounded set, <= 2*ARM_LIMIT) in a single query.
fn apply_scope_boost(conn: &Connection, score: &mut HashMap<i64, f32>) -> Result<()> {
    if score.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = score.keys().copied().collect();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT id, scope_kind FROM facts WHERE id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let binds: Vec<Value> = ids.iter().map(|i| Value::Integer(*i)).collect();
    let rows = stmt.query_map(params_from_iter(binds), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, kind) = row?;
        let boost = match kind.as_str() {
            "session" => SCOPE_BOOST_SESSION,
            "project" => SCOPE_BOOST_PROJECT,
            _ => 0.0,
        };
        if boost != 0.0 {
            if let Some(s) = score.get_mut(&id) {
                *s += boost;
            }
        }
    }
    Ok(())
}

/// Deprioritize (never exclude) cold-tier hits among the fused candidates, so `consolidate()`'s
/// warm->cold demotion has an actual, observable effect on ranking instead of being a label nobody
/// reads. Same bounded-lookup shape as `apply_scope_boost`.
fn apply_tier_penalty(conn: &Connection, score: &mut HashMap<i64, f32>) -> Result<()> {
    if score.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = score.keys().copied().collect();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT id, tier FROM facts WHERE id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let binds: Vec<Value> = ids.iter().map(|i| Value::Integer(*i)).collect();
    let rows = stmt.query_map(params_from_iter(binds), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, tier) = row?;
        if tier == "cold" {
            if let Some(s) = score.get_mut(&id) {
                *s -= TIER_PENALTY_COLD;
            }
        }
    }
    Ok(())
}

fn taint_str(t: Taint) -> &'static str {
    if t.is_untrusted() {
        "untrusted"
    } else {
        "trusted"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::TrigramHashEmbedder;

    fn mem() -> (Memory, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let embedder = Arc::new(TrigramHashEmbedder::default());
        let m = Memory::open(dir.path().join("brain.db"), embedder, ledger).unwrap();
        (m, dir)
    }

    #[test]
    fn access_bumps_batch_into_one_ledger_entry_per_minute() {
        let (m, _d) = mem();
        let a = m
            .remember(WriteReq::new(Region::Semantic, "alpha fact"))
            .unwrap();
        let before = m.ledger.tail(20).unwrap().len();

        // First access opens the window; below the 60s threshold, nothing flushes yet - the field
        // driving consolidation must still be usable without waiting on the ledger.
        m.record_access(a.id);
        assert_eq!(
            m.ledger.tail(20).unwrap().len(),
            before,
            "a fresh window must not flush immediately"
        );

        // Simulate a minute of elapsed activity by rewinding the open window's start, then push a
        // second access - this is what crosses the threshold in real use, just without a real sleep.
        {
            let mut batch = m.access_batch.lock().unwrap();
            batch.window_start_ms -= 61_000;
        }
        m.record_access(a.id);

        let entries = m.ledger.tail(20).unwrap();
        let flushed = entries
            .iter()
            .rfind(|e| e.kind == "memory.access_batch")
            .expect("crossing the window must append exactly one batched ledger entry");
        let count = flushed
            .payload
            .get()
            .parse::<serde_json::Value>()
            .unwrap()["count"]
            .as_u64()
            .unwrap();
        // The access that crosses the threshold is included in the SAME flush (no access is ever
        // silently dropped) - so both the first (window-opening) and second (threshold-crossing)
        // touches land in this one batch.
        assert_eq!(count, 2, "the flush must include every access accumulated up to and including the one that crossed the threshold");

        // The second access opened a fresh window and must not have flushed yet either.
        let after_first_flush = m.ledger.tail(20).unwrap().len();
        m.record_access(a.id);
        assert_eq!(
            m.ledger.tail(20).unwrap().len(),
            after_first_flush,
            "the new window must not flush again immediately"
        );
    }

    /// A test-only embedder that can be toggled between "healthy" and "degraded" (mimicking a
    /// gateway embedder that's fallen back after a provider outage), so `needs_reembed` marking
    /// and `reembed_flagged()`'s repair pass can be exercised deterministically.
    struct FlakyEmbedder {
        inner: TrigramHashEmbedder,
        healthy: std::sync::atomic::AtomicBool,
    }
    impl crate::embed::Embedder for FlakyEmbedder {
        fn dim(&self) -> usize {
            self.inner.dim()
        }
        fn embed(&self, text: &str) -> Vec<f32> {
            self.inner.embed(text)
        }
        fn name(&self) -> &str {
            "flaky-test-embedder"
        }
        fn embed_checked(&self, text: &str) -> (Vec<f32>, bool) {
            let degraded = !self.healthy.load(std::sync::atomic::Ordering::SeqCst);
            (self.inner.embed(text), degraded)
        }
    }

    #[test]
    fn a_degraded_write_is_flagged_and_reembed_flagged_repairs_it() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let embedder = Arc::new(FlakyEmbedder {
            inner: TrigramHashEmbedder::default(),
            healthy: std::sync::atomic::AtomicBool::new(false),
        });
        let m = Memory::open(dir.path().join("brain.db"), embedder.clone(), ledger).unwrap();

        let rec = m
            .remember(WriteReq::new(Region::Semantic, "written while degraded"))
            .unwrap();
        let stats = m.stats().unwrap();
        assert_eq!(stats.needs_reembed, 1, "a degraded write must be flagged");

        // Still degraded: reembed_flagged() must leave the flag set rather than clearing it on a
        // vector that's just as degraded as the one already stored.
        assert_eq!(m.reembed_flagged().unwrap(), 0);
        assert_eq!(m.stats().unwrap().needs_reembed, 1);

        // Provider recovers: the next pass must clear the flag on the SAME row.
        embedder
            .healthy
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(m.reembed_flagged().unwrap(), 1);
        assert_eq!(m.stats().unwrap().needs_reembed, 0);

        // The row itself is untouched otherwise and still recallable.
        let after = get_record(&m.conn.lock().unwrap(), rec.id).unwrap().unwrap();
        assert_eq!(after.text, "written while degraded");

        // A normal (non-degraded) write on the same brain is never flagged.
        embedder
            .healthy
            .store(true, std::sync::atomic::Ordering::SeqCst);
        m.remember(WriteReq::new(Region::Semantic, "written while healthy"))
            .unwrap();
        assert_eq!(m.stats().unwrap().needs_reembed, 0);
    }

    #[test]
    fn remember_then_recall_keyword() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(
            Region::Semantic,
            "Engram runs on a cheap VPS",
        ))
        .unwrap();
        let hits = m.recall("cheap VPS", &[Region::Semantic], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].record.text.contains("VPS"));
        assert!(hits[0].keyword_rank.is_some());
    }

    #[test]
    fn dedup_on_write_bumps_instead_of_duplicating() {
        let (m, _d) = mem();
        let fact = "The user prefers concise answers";
        let a = m
            .remember(WriteReq::new(Region::Identity, fact).importance(0.4))
            .unwrap();
        // Re-learning the SAME fact must not create a second row; it bumps the existing one.
        let b = m
            .remember(WriteReq::new(Region::Identity, fact).importance(0.9))
            .unwrap();
        assert_eq!(a.id, b.id, "the duplicate must merge into the same row");
        assert!(
            (b.importance - 0.9).abs() < 1e-6,
            "importance bumps to the max seen"
        );
        assert!(b.access_count >= 1, "access count reinforced");
        // Recall returns exactly ONE row for that fact (not two).
        let hits = m.recall("concise answers", &[Region::Identity], 5).unwrap();
        assert_eq!(hits.len(), 1, "deduped fact appears once in recall");
        // A genuinely different fact in the same region is a separate row.
        let c = m
            .remember(WriteReq::new(Region::Identity, "The user works in Rust"))
            .unwrap();
        assert_ne!(c.id, a.id);
    }

    #[test]
    fn dedup_respects_taint_provenance() {
        let (m, _d) = mem();
        let fact = "the api token rotates every 24 hours";
        // First captured UNTRUSTED (e.g. during a web-tainted run).
        let u = m
            .remember(WriteReq::new(Region::Semantic, fact).taint(Taint::Untrusted))
            .unwrap();
        assert_eq!(u.taint, "untrusted");
        // Not visible to trusted recall yet.
        assert!(m
            .recall_trusted(fact, &[Region::Semantic], 5)
            .unwrap()
            .is_empty());
        // A TRUSTED re-assertion of the identical text must UPGRADE the row, not be discarded.
        let t = m
            .remember(WriteReq::new(Region::Semantic, fact).taint(Taint::Trusted))
            .unwrap();
        assert_eq!(t.id, u.id, "still one row (deduped)");
        assert_eq!(t.taint, "trusted", "trusted assertion upgrades provenance");
        // Now visible to trusted recall.
        assert!(!m
            .recall_trusted(fact, &[Region::Semantic], 5)
            .unwrap()
            .is_empty());

        // Conversely: an untrusted write must NOT inflate a trusted row's salience.
        let tfact = "the user prefers metric units";
        let base = m
            .remember(WriteReq::new(Region::Semantic, tfact).importance(0.3))
            .unwrap();
        let before = m.get(base.id).unwrap().unwrap();
        let bumped = m
            .remember(
                WriteReq::new(Region::Semantic, tfact)
                    .importance(0.99)
                    .taint(Taint::Untrusted),
            )
            .unwrap();
        assert_eq!(bumped.id, base.id, "still one row (deduped)");
        assert_eq!(bumped.taint, "trusted", "trusted row stays trusted");
        let after = m.get(base.id).unwrap().unwrap();
        assert!(
            (after.importance - before.importance).abs() < 1e-6,
            "an untrusted write must not raise a trusted row's importance"
        );
        assert_eq!(
            after.access_count, before.access_count,
            "an untrusted write must not bump a trusted row's access count"
        );
    }

    #[test]
    fn scope_defaults_to_user_and_round_trips() {
        let (m, _d) = mem();
        // A default write is user-global.
        let g = m
            .remember(WriteReq::new(Region::Semantic, "global fact"))
            .unwrap();
        assert_eq!(g.scope_kind, "user");
        assert_eq!(g.scope_id, "");
        // A project-scoped write round-trips its scope on the returned record and on re-fetch.
        let p = m
            .remember(WriteReq::new(Region::Semantic, "project fact").scope(Scope::project("p1")))
            .unwrap();
        assert_eq!(p.scope_kind, "project");
        assert_eq!(p.scope_id, "p1");
        let refetched = m.get(p.id).unwrap().unwrap();
        assert_eq!(refetched.scope_kind, "project");
        assert_eq!(refetched.scope_id, "p1");
        // The SAME text in a different scope is a DISTINCT row (scope-aware dedup), not a bump.
        let same_text_other_scope = m
            .remember(WriteReq::new(Region::Semantic, "global fact").scope(Scope::project("p1")))
            .unwrap();
        assert_ne!(
            same_text_other_scope.id, g.id,
            "same text in a different ring must be its own row"
        );
        // But re-writing the SAME text in the SAME scope still dedups (bumps, one row).
        let dup = m
            .remember(WriteReq::new(Region::Semantic, "global fact"))
            .unwrap();
        assert_eq!(dup.id, g.id, "same text in the same ring dedups");
    }

    #[test]
    fn scoped_recall_isolates_projects_but_keeps_user_global() {
        let (m, _d) = mem();
        // Two projects each with their own deploy-target fact, plus a user-global identity fact.
        m.remember(
            WriteReq::new(Region::Semantic, "the deploy target is fly.io")
                .scope(Scope::project("A")),
        )
        .unwrap();
        m.remember(
            WriteReq::new(Region::Semantic, "the deploy target is render")
                .scope(Scope::project("B")),
        )
        .unwrap();
        m.remember(WriteReq::new(
            Region::Identity,
            "the user prefers concise answers",
        ))
        .unwrap(); // user-global by default

        // Inside project B: sees B's fact, NEVER project A's (the bleed, fixed).
        let ctx_b = ScopeCtx::project("B");
        let hits = m
            .recall_trusted_scoped("deploy target", &[Region::Semantic], 5, &ctx_b)
            .unwrap();
        assert!(
            hits.iter().any(|h| h.record.text.contains("render")),
            "project B recalls its own fact"
        );
        assert!(
            !hits.iter().any(|h| h.record.text.contains("fly.io")),
            "project B must NOT recall project A's fact"
        );

        // The user-global identity fact still surfaces inside project B.
        let ident = m
            .recall_trusted_scoped("concise answers", &[Region::Identity], 5, &ctx_b)
            .unwrap();
        assert!(
            ident.iter().any(|h| h.record.text.contains("concise")),
            "user-global memory follows the user into every project"
        );

        // A brand-new project C starts clean: no other project's work, only user-global.
        let ctx_c = ScopeCtx::project("C");
        let deploy_c = m
            .recall_trusted_scoped("deploy target", &[Region::Semantic], 5, &ctx_c)
            .unwrap();
        assert!(
            deploy_c
                .iter()
                .all(|h| !h.record.text.contains("fly.io") && !h.record.text.contains("render")),
            "a new project sees no other project's work"
        );
    }

    #[test]
    fn specificity_boost_prefers_project_over_global_on_a_tie() {
        let (m, _d) = mem();
        // Identical text in the user ring and in project P: with project P active, the project
        // copy should rank first (the specificity tie-break).
        let _g = m
            .remember(WriteReq::new(Region::Semantic, "the api base url is set"))
            .unwrap();
        let p = m
            .remember(
                WriteReq::new(Region::Semantic, "the api base url is set")
                    .scope(Scope::project("P")),
            )
            .unwrap();
        let hits = m
            .recall_scoped(
                "api base url",
                &[Region::Semantic],
                5,
                &ScopeCtx::project("P"),
            )
            .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(
            hits[0].record.id, p.id,
            "the project-ringed copy wins the tie over the user-global one"
        );
        assert_eq!(hits[0].record.scope_kind, "project");
    }

    #[test]
    fn low_importance_old_memory_is_still_recalled_by_paraphrase() {
        // The completeness fix: the old salience cap made only the top-N-by-importance rows
        // eligible for the semantic arm, so a low-importance but semantically-perfect memory could
        // be silently invisible. The binary coarse pass scans ALL in-scope rows, so it surfaces.
        let (m, _d) = mem();
        // Bury the target under many higher-importance, unrelated memories.
        for i in 0..60 {
            m.remember(
                WriteReq::new(
                    Region::Semantic,
                    format!("unrelated note number {i} about invoices"),
                )
                .importance(0.95),
            )
            .unwrap();
        }
        let target = m
            .remember(
                WriteReq::new(
                    Region::Semantic,
                    "the user preferences include a dark theme",
                )
                .importance(0.05),
            )
            .unwrap();
        let hits = m
            .recall("preferred theming", &[Region::Semantic], 5)
            .unwrap();
        assert!(
            hits.iter().any(|h| h.record.id == target.id),
            "the low-importance paraphrase match must still be recalled"
        );
    }

    #[test]
    fn paraphrase_survives_coarse_pass_in_a_large_ring() {
        // Regression for the binary-coarse discriminator bug: the sparse default embedder makes
        // Hamming a weak filter, so short unrelated rows could crowd out a genuine paraphrase once a
        // ring exceeded the coarse truncation (256). Seed >1000 short noise rows plus one longer
        // paraphrase target and assert recall still surfaces it. This crosses both COARSE_K and (with
        // the seeded count) stays under EXACT_SCAN_MAX, exercising the skip-coarse exact path.
        let (m, _d) = mem();
        for i in 0..1200 {
            m.remember(WriteReq::new(Region::Semantic, format!("note {i}")))
                .unwrap();
        }
        let target = m
            .remember(WriteReq::new(
                Region::Semantic,
                "the user strongly prefers a dark theme in the code editor at night",
            ))
            .unwrap();
        let hits = m
            .recall("preferred dark theming editor", &[Region::Semantic], 5)
            .unwrap();
        assert!(
            hits.iter().any(|h| h.record.id == target.id),
            "the paraphrase target must survive the coarse pass even in a >1000-row ring"
        );
    }

    #[test]
    fn recent_scoped_omits_superseded_facts() {
        // The stale-fact guard (the "Mondaine vs Omega" class): once a fact is superseded, it must
        // NOT be a distillation candidate. recent_scoped feeds consciousness's AUTHORITATIVE
        // working-memory block, so a leaked superseded fact would override the current truth.
        let (m, _d) = mem();
        let old = m
            .remember(WriteReq::new(Region::Identity, "the user lives in Berlin"))
            .unwrap();
        let new = m
            .remember(WriteReq::new(Region::Identity, "the user lives in Munich"))
            .unwrap();
        assert!(m.supersede(old.id, new.id).unwrap());

        let recent = m
            .recent_scoped(Region::Identity, 50, &ScopeCtx::any())
            .unwrap();
        assert!(
            recent.iter().any(|r| r.text.contains("Munich")),
            "the current fact is still recalled"
        );
        assert!(
            !recent.iter().any(|r| r.text.contains("Berlin")),
            "the superseded (stale) fact must NOT be a distillation candidate"
        );
        // recent() (chat reload / Atlas working-memory) delegates here, so it is guarded too.
        let any = m.recent(Region::Identity, 50).unwrap();
        assert!(!any.iter().any(|r| r.text.contains("Berlin")));
    }

    #[test]
    fn reindex_binary_rebuilds_the_coarse_index_from_source_of_truth() {
        let (m, _d) = mem();
        for i in 0..5 {
            m.remember(WriteReq::new(Region::Semantic, format!("fact number {i}")))
                .unwrap();
        }
        m.remember(WriteReq::new(
            Region::Semantic,
            "the user prefers a dark theme",
        ))
        .unwrap();
        // Wipe the derived binary index entirely.
        {
            let conn = m.conn.lock().unwrap();
            conn.execute("UPDATE facts SET embedding_bin = NULL", [])
                .unwrap();
        }
        // Reindex rebuilds every code from the stored embeddings (the source of truth).
        let n = m.reindex_binary().unwrap();
        assert_eq!(n, 6, "every row is reindexed");
        // Recall still works and finds the paraphrase.
        let hits = m
            .recall("preferred theming", &[Region::Semantic], 5)
            .unwrap();
        assert!(
            hits.iter().any(|h| h.record.text.contains("dark theme")),
            "recall works after a full reindex"
        );
    }

    #[test]
    fn promote_to_user_makes_a_project_fact_global_and_guards_untrusted() {
        let (m, _d) = mem();
        let p = m
            .remember(WriteReq::new(Region::Semantic, "always use pnpm").scope(Scope::project("P")))
            .unwrap();
        // Before promotion, project Q can't see P's fact.
        assert!(m
            .recall_trusted_scoped("pnpm", &[Region::Semantic], 5, &ScopeCtx::project("Q"))
            .unwrap()
            .is_empty());
        assert!(m.promote_to_user(p.id, "user").unwrap());
        // After promotion it is user-global and surfaces in any project.
        assert_eq!(m.get(p.id).unwrap().unwrap().scope_kind, "user");
        assert!(!m
            .recall_trusted_scoped("pnpm", &[Region::Semantic], 5, &ScopeCtx::project("Q"))
            .unwrap()
            .is_empty());
        // Untrusted-provenance memory refuses promotion into the trusted user-global ring.
        let u = m
            .remember(
                WriteReq::new(Region::Semantic, "a scraped claim")
                    .taint(Taint::Untrusted)
                    .scope(Scope::project("P")),
            )
            .unwrap();
        assert!(m.promote_to_user(u.id, "user").is_err());
    }

    #[test]
    fn whole_brain_recall_still_sees_all_scopes() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(Region::Semantic, "alpha fact").scope(Scope::project("A")))
            .unwrap();
        m.remember(WriteReq::new(Region::Semantic, "beta fact").scope(Scope::project("B")))
            .unwrap();
        // The bare recall() (Atlas / audit view) spans every scope.
        let hits = m.recall("fact", &[Region::Semantic], 10).unwrap();
        assert!(hits.iter().any(|h| h.record.text.contains("alpha")));
        assert!(hits.iter().any(|h| h.record.text.contains("beta")));
    }

    #[test]
    fn semantic_recall_finds_what_keyword_misses() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(
            Region::Identity,
            "the user preferences include a dark theme",
        ))
        .unwrap();
        m.remember(WriteReq::new(
            Region::Identity,
            "the weather in Berlin is cold today",
        ))
        .unwrap();

        // No shared whole-word tokens with the stored text ("preferred"/"theming"
        // are different tokens than "preferences"/"theme"), so keyword search alone
        // returns nothing - yet hybrid recall still surfaces the right memory.
        assert!(build_match("preferred theming")
            .map(|q| !q.contains("preferences"))
            .unwrap_or(true));
        let hits = m
            .recall("preferred theming", &[Region::Identity], 3)
            .unwrap();
        assert!(!hits.is_empty(), "hybrid recall should find the paraphrase");
        assert!(hits[0].record.text.contains("preferences"));
        assert!(
            hits[0].keyword_rank.is_none(),
            "semantic arm, not keyword, should have carried this hit"
        );
        assert!(hits[0].semantic_rank.is_some());
    }

    #[test]
    fn recall_respects_regions() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(
            Region::Identity,
            "favourite language is Rust",
        ))
        .unwrap();
        m.remember(WriteReq::new(
            Region::Episodic,
            "favourite language is Rust",
        ))
        .unwrap();
        let hits = m
            .recall("favourite language", &[Region::Identity], 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.region, "identity");
    }

    #[test]
    fn recall_trusted_excludes_untrusted_provenance() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(
            Region::Semantic,
            "Engram deploys to a cheap VPS",
        ))
        .unwrap();
        // Content read during a tainted run (e.g. scraped from an attacker page) inherits
        // Untrusted provenance.
        m.remember(
            WriteReq::new(
                Region::Semantic,
                "Engram deploys to a cheap VPS, per a web page",
            )
            .taint(Taint::Untrusted),
        )
        .unwrap();

        // The transparency recall sees both…
        assert_eq!(
            m.recall("cheap VPS", &[Region::Semantic], 5).unwrap().len(),
            2
        );
        // …but model-facing recall returns only the trusted one - injected memory can't
        // re-surface as trusted context and poison a clean run.
        let trusted = m
            .recall_trusted("cheap VPS", &[Region::Semantic], 5)
            .unwrap();
        assert_eq!(trusted.len(), 1);
        assert_eq!(trusted[0].record.taint, "trusted");
    }

    #[test]
    fn switching_embedding_space_re_embeds_existing_memories() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let path = dir.path().join("brain.db");
        {
            let m = Memory::open(
                &path,
                Arc::new(TrigramHashEmbedder::new(256)),
                ledger.clone(),
            )
            .unwrap();
            m.remember(WriteReq::new(
                Region::Semantic,
                "Engram runs on a cheap VPS",
            ))
            .unwrap();
        }
        // Reopen under a different-dimension embedder - a new embedding space.
        let m = Memory::open(
            &path,
            Arc::new(TrigramHashEmbedder::new(128)),
            ledger.clone(),
        )
        .unwrap();
        // The migration ran and is recorded…
        assert!(ledger
            .read_all()
            .unwrap()
            .iter()
            .any(|e| e.kind == "memory.reembed"));
        // …and recall still works against the re-embedded vectors.
        assert_eq!(
            m.recall("cheap VPS", &[Region::Semantic], 5).unwrap().len(),
            1
        );
        // Reopening with the SAME embedder is a no-op - no second migration.
        let before = ledger
            .read_all()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "memory.reembed")
            .count();
        let _again = Memory::open(
            &path,
            Arc::new(TrigramHashEmbedder::new(128)),
            ledger.clone(),
        )
        .unwrap();
        let after = ledger
            .read_all()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "memory.reembed")
            .count();
        assert_eq!(
            before, after,
            "no migration when the embedding space is unchanged"
        );
    }

    #[test]
    fn supersede_makes_the_old_fact_history_not_recalled() {
        let (m, _d) = mem();
        let berlin = m
            .remember(WriteReq::new(Region::Identity, "User lives Berlin"))
            .unwrap();
        let munich = m
            .remember(WriteReq::new(Region::Identity, "User lives Munich"))
            .unwrap();
        // Before supersession both surface.
        assert_eq!(
            m.recall("where the user lives", &[Region::Identity], 5)
                .unwrap()
                .len(),
            2
        );

        assert!(m.supersede(berlin.id, munich.id).unwrap());

        // Only the current truth recalls - the stale fact can't be confidently wrong.
        let hits = m
            .recall("where the user lives", &[Region::Identity], 5)
            .unwrap();
        assert!(
            hits.iter().all(|h| h.record.id != berlin.id),
            "superseded fact must not recall"
        );
        assert!(hits.iter().any(|h| h.record.id == munich.id));
        // The prefix lookup now sees only the current value.
        assert_eq!(
            m.current_with_prefix(Region::Identity, "User lives ")
                .unwrap(),
            vec![munich.id]
        );
        // Superseding a non-current id is a no-op.
        assert!(!m.supersede(berlin.id, munich.id).unwrap());
        // Ledger-first: the supersede stamped the old row's ledger_seq (audit trail present).
        assert!(
            m.get(berlin.id).unwrap().unwrap().ledger_seq.is_some(),
            "supersede must leave a ledger reference on the superseded row"
        );
    }

    #[test]
    fn forget_then_restore() {
        let (m, _d) = mem();
        let rec = m
            .remember(WriteReq::new(Region::Semantic, "secret to forget"))
            .unwrap();
        assert!(m.forget(rec.id, "user", "no longer relevant").unwrap());
        assert!(m
            .recall("secret", &[Region::Semantic], 5)
            .unwrap()
            .is_empty());
        assert!(m.restore(rec.id, "user").unwrap());
        assert_eq!(m.recall("secret", &[Region::Semantic], 5).unwrap().len(), 1);
    }

    #[test]
    fn forget_scope_cascades_a_project_but_spares_user_global() {
        let (m, _d) = mem();
        m.remember(
            WriteReq::new(Region::Semantic, "project A fact one").scope(Scope::project("A")),
        )
        .unwrap();
        m.remember(
            WriteReq::new(Region::Semantic, "project A fact two").scope(Scope::project("A")),
        )
        .unwrap();
        m.remember(WriteReq::new(Region::Semantic, "project B fact").scope(Scope::project("B")))
            .unwrap();
        m.remember(WriteReq::new(Region::Semantic, "user global fact"))
            .unwrap();
        // Deleting project A forgets exactly its two facts.
        let n = m
            .forget_scope("project", "A", "user", "project deleted")
            .unwrap();
        assert_eq!(n, 2);
        // Idempotent: a second sweep finds nothing live.
        assert_eq!(m.forget_scope("project", "A", "user", "again").unwrap(), 0);
        // Project B and the user-global ring are untouched.
        assert_eq!(m.forget_scope("project", "B", "user", "x").unwrap(), 1);
        // The user ring can never be mass-forgotten by this call.
        assert_eq!(m.forget_scope("user", "", "user", "x").unwrap(), 0);
        assert_eq!(m.forget_scope("project", "", "user", "x").unwrap(), 0);
    }

    #[test]
    fn writes_are_ledgered_and_chain_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let m = Memory::open(
            dir.path().join("brain.db"),
            Arc::new(TrigramHashEmbedder::default()),
            ledger.clone(),
        )
        .unwrap();
        let rec = m
            .remember(WriteReq::new(Region::Semantic, "tracked fact"))
            .unwrap();
        assert!(rec.ledger_seq.is_some());
        assert!(ledger.verify().unwrap() >= 1);
    }

    #[test]
    fn consolidation_demotes_stale_low_importance() {
        let (m, _d) = mem();
        let rec = m
            .remember(WriteReq::new(Region::Episodic, "trivial chatter").importance(0.1))
            .unwrap();
        // Age the row past the warm window.
        {
            let conn = m.conn.lock().unwrap();
            conn.execute(
                "UPDATE facts SET last_access_ms = 0 WHERE id = ?1",
                [rec.id],
            )
            .unwrap();
        }
        let demoted = m.consolidate(Duration::from_secs(60)).unwrap();
        assert_eq!(demoted, 1);
        assert_eq!(m.get(rec.id).unwrap().unwrap().tier, "cold");
    }

    #[test]
    fn find_similar_not_identical_excludes_exact_duplicates_and_low_similarity() {
        let (m, _d) = mem();
        let scope = Scope::user();
        m.remember(WriteReq::new(Region::Semantic, "the deploy pipeline runs on port 9090"))
            .unwrap();
        m.remember(WriteReq::new(Region::Semantic, "the cafeteria menu changes on Tuesdays"))
            .unwrap();

        // An exact duplicate must never show up as a "candidate contradiction" - that's dedup's job.
        let exact = m
            .find_similar_not_identical(
                Region::Semantic,
                &scope,
                "the deploy pipeline runs on port 9090",
                0.0,
                5,
            )
            .unwrap();
        assert!(
            exact.iter().all(|r| r.text != "the deploy pipeline runs on port 9090"),
            "an exact-text match must be excluded (it's a dedup case, not a contradiction candidate)"
        );

        // A near-paraphrase above threshold IS a candidate; an unrelated fact below threshold isn't.
        let near = m
            .find_similar_not_identical(
                Region::Semantic,
                &scope,
                "the deploy pipeline actually runs on port 8080 now",
                0.3,
                5,
            )
            .unwrap();
        assert!(
            near.iter().any(|r| r.text.contains("9090")),
            "a genuine near-duplicate must be found as a candidate"
        );
        assert!(
            near.iter().all(|r| !r.text.contains("cafeteria")),
            "an unrelated fact must not pass the similarity threshold"
        );
    }

    #[test]
    fn reflection_candidates_groups_related_stale_trusted_facts_and_excludes_the_rest() {
        let (m, _d) = mem();
        let related = [
            "the payment gateway staging config uses TLS 1.2 certificates",
            "the payment gateway staging config was migrated to TLS 1.3 certificates",
            "the payment gateway staging environment enforces strict TLS certificate checks",
        ];
        let mut related_ids = Vec::new();
        for text in related {
            let rec = m
                .remember(WriteReq::new(Region::Semantic, text).importance(0.2))
                .unwrap();
            related_ids.push(rec.id);
        }
        // An untrusted-provenance near-duplicate of the SAME topic must never enter a reflection
        // group - a reflection is a Trusted-only synthesis.
        let untrusted = m
            .remember(
                WriteReq::new(
                    Region::Semantic,
                    "the payment gateway staging config maybe uses TLS certificates too",
                )
                .importance(0.2)
                .taint(Taint::Untrusted),
            )
            .unwrap();
        // An unrelated fact must not be pulled into the same cluster.
        let unrelated = m
            .remember(WriteReq::new(Region::Semantic, "the cafeteria menu changes on Tuesdays").importance(0.2))
            .unwrap();
        // A high-importance fact is never a demotion/reflection candidate at all, related or not.
        let important = m
            .remember(
                WriteReq::new(
                    Region::Semantic,
                    "the payment gateway staging config also uses TLS certificates",
                )
                .importance(0.9),
            )
            .unwrap();

        // Age every row past the warm window (reflection reuses consolidate's exact predicate).
        {
            let conn = m.conn.lock().unwrap();
            conn.execute("UPDATE facts SET last_access_ms = 0", []).unwrap();
        }

        let groups = m
            .reflection_candidates(Duration::from_secs(60), 0.3, 5)
            .unwrap();
        assert_eq!(groups.len(), 1, "exactly one related cluster should form");
        let ids: Vec<i64> = groups[0].iter().map(|r| r.id).collect();
        for id in &related_ids {
            assert!(ids.contains(id), "every related fact must be in the cluster");
        }
        assert!(
            !ids.contains(&untrusted.id),
            "an untrusted-provenance fact must never enter a reflection candidate group"
        );
        assert!(
            !ids.contains(&unrelated.id),
            "an unrelated fact must not be pulled into the cluster"
        );
        assert!(
            !ids.contains(&important.id),
            "a high-importance fact is never a demotion/reflection candidate"
        );
    }

    #[test]
    fn pending_supersession_is_never_applied_until_resolved() {
        let (m, _d) = mem();
        let old = m
            .remember(WriteReq::new(Region::Semantic, "the API base url is api.old.example"))
            .unwrap();
        let pid = m
            .propose_supersession(
                old.id,
                "the API base url is api.new.example",
                "the user said the domain moved",
                Region::Semantic,
                &Scope::user(),
                "core",
            )
            .unwrap();

        // Proposing must NOT touch the original fact or write anything new - mandatory
        // confirmation, no silent auto-apply (the locked decision in the plan's §5).
        assert_eq!(m.recall("api base url", &[Region::Semantic], 5).unwrap().len(), 1);
        assert_eq!(m.get(old.id).unwrap().unwrap().text, "the API base url is api.old.example");
        let pending = m.pending_supersessions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, pid);
        assert_eq!(pending[0].old_id, old.id);

        // Rejecting resolves it with no effect at all.
        assert!(m.resolve_supersession(pid, false, "user").unwrap());
        assert!(m.pending_supersessions().unwrap().is_empty());
        assert_eq!(m.get(old.id).unwrap().unwrap().text, "the API base url is api.old.example");
        assert_eq!(m.recall("api base url", &[Region::Semantic], 5).unwrap().len(), 1);

        // A second proposal, accepted THIS time, writes the new fact and supersedes the old one -
        // through the same remember()/supersede() every other write path already uses.
        let pid2 = m
            .propose_supersession(
                old.id,
                "the API base url is api.new.example",
                "the user said the domain moved",
                Region::Semantic,
                &Scope::user(),
                "core",
            )
            .unwrap();
        assert!(m.resolve_supersession(pid2, true, "user").unwrap());
        let hits = m.recall("api base url", &[Region::Semantic], 5).unwrap();
        assert_eq!(hits.len(), 1, "the old fact must no longer surface once accepted");
        assert!(hits[0].record.text.contains("new.example"));
        assert!(m.pending_supersessions().unwrap().is_empty());

        // Resolving an already-resolved (or unknown) id is a no-op, not an error or a double-apply.
        assert!(!m.resolve_supersession(pid2, true, "user").unwrap());
        assert!(!m.resolve_supersession(999_999, true, "user").unwrap());
    }

    #[test]
    fn recall_as_of_answers_what_was_true_at_a_past_moment() {
        let (m, _d) = mem();
        let berlin = m
            .remember(WriteReq::new(Region::Identity, "User lives Berlin"))
            .unwrap();
        let munich = m
            .remember(WriteReq::new(Region::Identity, "User lives Munich"))
            .unwrap();
        assert!(m.supersede(berlin.id, munich.id).unwrap());

        // Give the pair clean, well-separated fabricated timestamps (real wall-clock writes in a
        // fast test all cluster within milliseconds, which would make "before"/"after" meaningless).
        {
            let conn = m.conn.lock().unwrap();
            conn.execute(
                "UPDATE facts SET valid_from_ms = 1000, valid_until_ms = 2000 WHERE id = ?1",
                [berlin.id],
            )
            .unwrap();
            conn.execute(
                "UPDATE facts SET valid_from_ms = 2000, valid_until_ms = NULL WHERE id = ?1",
                [munich.id],
            )
            .unwrap();
        }

        let scope = ScopeCtx::user_only();
        // As of t=1500 (after Berlin, before Munich): only Berlin was true.
        let as_of_early = m
            .recall_as_of("where the user lives", &[Region::Identity], 5, &scope, 1500)
            .unwrap();
        assert!(as_of_early.iter().any(|h| h.record.id == berlin.id));
        assert!(as_of_early.iter().all(|h| h.record.id != munich.id));

        // As of t=2500 (after the switch): only Munich.
        let as_of_late = m
            .recall_as_of("where the user lives", &[Region::Identity], 5, &scope, 2500)
            .unwrap();
        assert!(as_of_late.iter().any(|h| h.record.id == munich.id));
        assert!(as_of_late.iter().all(|h| h.record.id != berlin.id));

        // The default (non-as-of) recall path is untouched: exactly the current version, same as
        // every existing recall test already asserts - re-confirmed here alongside the new feature.
        let current = m
            .recall("where the user lives", &[Region::Identity], 5)
            .unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].record.id, munich.id);
    }

    #[test]
    fn by_source_scoped_finds_exact_matches_in_order_and_respects_scope() {
        let (m, _d) = mem();
        let a = m
            .remember(WriteReq::new(Region::Episodic, "page one").source("compaction:7"))
            .unwrap();
        let b = m
            .remember(WriteReq::new(Region::Episodic, "page two").source("compaction:7"))
            .unwrap();
        m.remember(WriteReq::new(Region::Episodic, "unrelated page").source("compaction:8"))
            .unwrap();
        m.remember(
            WriteReq::new(Region::Episodic, "a different project's page")
                .source("compaction:7")
                .scope(Scope::project("P")),
        )
        .unwrap();

        let pages = m
            .by_source_scoped("compaction:7", &ScopeCtx::user_only())
            .unwrap();
        assert_eq!(pages.len(), 2, "only this exact source, in the user ring");
        assert_eq!(pages[0].id, a.id, "oldest first");
        assert_eq!(pages[1].id, b.id);
        assert!(pages.iter().all(|p| p.source.as_deref() == Some("compaction:7")));
    }

    #[test]
    fn actor_is_persisted_and_queryable_per_scope() {
        let (m, _d) = mem();
        m.remember(
            WriteReq::new(Region::Semantic, "Atlas's note about the deploy pipeline")
                .actor("agent:Atlas")
                .scope(Scope::project("P")),
        )
        .unwrap();
        m.remember(
            WriteReq::new(Region::Semantic, "a user fact")
                .actor("user")
                .scope(Scope::project("P")),
        )
        .unwrap();
        m.remember(WriteReq::new(Region::Semantic, "unrelated global fact").actor("user"))
            .unwrap();

        // Attribution round-trips on the record itself - no ledger cross-reference needed.
        let hits = m
            .recall_scoped(
                "Atlas deploy",
                &[Region::Semantic],
                5,
                &ScopeCtx::project("P"),
            )
            .unwrap();
        let atlas_hit = hits
            .iter()
            .find(|h| h.record.text.contains("Atlas's note"))
            .expect("Atlas's memory must be recallable from the shared project ring");
        assert_eq!(atlas_hit.record.actor, "agent:Atlas");

        // Per-project stats break down by actor, scoped to just that ring.
        let stats = m.stats_for_scope("project", "P").unwrap();
        assert_eq!(stats.total, 2, "only P's own two facts, not the unrelated global one");
        assert_eq!(stats.by_actor.get("agent:Atlas"), Some(&1));
        assert_eq!(stats.by_actor.get("user"), Some(&1));

        // The global brain-wide stats still see all three, and the user-only ring sees just one.
        assert_eq!(m.stats().unwrap().total, 3);
        let user_ring = m.stats_for_scope("user", "").unwrap();
        assert_eq!(user_ring.total, 1);
    }

    #[test]
    fn auto_prune_only_removes_old_already_superseded_rows() {
        let (m, _d) = mem();
        // Old + superseded: eligible.
        let old_super = m
            .remember(WriteReq::new(Region::Identity, "User lives Berlin"))
            .unwrap();
        let replacement = m
            .remember(WriteReq::new(Region::Identity, "User lives Munich"))
            .unwrap();
        m.supersede(old_super.id, replacement.id).unwrap();
        // Old but NOT superseded (still the current fact): must survive regardless of age.
        let old_current = m
            .remember(WriteReq::new(Region::Semantic, "still true old fact"))
            .unwrap();
        // Recently superseded: too young to prune even though it's stale.
        let young_super = m
            .remember(WriteReq::new(Region::Identity, "User works at Acme"))
            .unwrap();
        let young_replacement = m
            .remember(WriteReq::new(Region::Identity, "User works at Globex"))
            .unwrap();
        m.supersede(young_super.id, young_replacement.id).unwrap();

        // Rewind created_ms for the two rows meant to look old (200 days back); leave young_super
        // at its real (just-created) timestamp so it's correctly too recent to prune.
        {
            let conn = m.conn.lock().unwrap();
            let ancient = now_ms() as i64 - 200 * 24 * 3600 * 1000;
            for id in [old_super.id, old_current.id] {
                conn.execute(
                    "UPDATE facts SET created_ms = ?1 WHERE id = ?2",
                    params![ancient, id],
                )
                .unwrap();
            }
        }

        let pruned = m.auto_prune(Duration::from_secs(180 * 24 * 3600), "core").unwrap();
        assert_eq!(pruned, 1, "only the old, already-superseded row is eligible");
        assert!(m.get(old_super.id).unwrap().is_none(), "the eligible row is gone");
        assert!(
            m.get(old_current.id).unwrap().is_some(),
            "an old but still-current fact must never be pruned just for being old"
        );
        assert!(
            m.get(young_super.id).unwrap().is_some(),
            "a superseded row younger than the retention window must survive"
        );
    }

    #[test]
    fn cold_tier_is_deprioritized_but_still_recallable() {
        let (m, _d) = mem();
        // Identical text in two different scopes (so dedup-on-write, which is scope-aware, doesn't
        // merge them into one row) - on a tie everywhere else, only tier should decide the order.
        let warm = m
            .remember(WriteReq::new(Region::Semantic, "the deploy runbook lives in docs"))
            .unwrap();
        let cold = m
            .remember(
                WriteReq::new(Region::Semantic, "the deploy runbook lives in docs")
                    .scope(Scope::project("P")),
            )
            .unwrap();
        {
            let conn = m.conn.lock().unwrap();
            conn.execute("UPDATE facts SET tier = 'cold' WHERE id = ?1", [cold.id])
                .unwrap();
        }
        // Whole-brain recall (ScopeCtx::any()) so the scope-specificity boost never fires - tier is
        // the only thing left that can break the tie.
        let hits = m
            .recall("deploy runbook docs", &[Region::Semantic], 5)
            .unwrap();
        assert_eq!(hits.len(), 2, "the cold row must still be recallable, not excluded");
        assert_eq!(
            hits[0].record.id, warm.id,
            "a cold-tier hit must rank below an otherwise-tied warm one"
        );
        assert!(
            hits.iter().any(|h| h.record.id == cold.id),
            "the cold row must still appear in the results"
        );
    }
}
