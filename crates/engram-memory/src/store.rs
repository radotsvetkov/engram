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
/// MMR diversity/relevance trade-off for the semantic arm: relevance-dominant, but enough novelty
/// pressure to break up near-duplicate passages.
const MMR_LAMBDA: f32 = 0.7;
/// Reciprocal Rank Fusion constant (standard default).
const RRF_K: f32 = 60.0;

const COLS: &str =
    "id,region,text,importance,taint,tier,source,metadata,content_hash,ledger_seq,created_ms,last_access_ms,access_count,scope_kind,scope_id";

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("ledger: {0}")]
    Ledger(#[from] engram_core::LedgerError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
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
}

/// The brain on disk.
pub struct Memory {
    conn: Mutex<Connection>,
    embedder: Arc<dyn Embedder>,
    ledger: Arc<Ledger>,
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
        };
        mem.migrate_embedding_space()?;
        mem.backfill_binary()?;
        Ok(mem)
    }

    /// Populate `embedding_bin` for rows written before the binary index existed, computing each
    /// code from the stored `embedding` (no re-embedding needed). Bounded and idempotent: once every
    /// row has a code this is a cheap no-op. (P5 makes this resumable/off-boot for very large brains.)
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
        // Gather the live rows that need re-embedding (collect first so the statement is
        // dropped before we open the write transaction).
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn.prepare("SELECT id, text FROM facts WHERE deleted = 0")?;
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
        let embedding = self.embedder.embed(&req.text);
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
        let existing: Option<(i64, f64)> = conn
            .query_row(
                "SELECT id, importance FROM facts WHERE region = ?1 AND content_hash = ?2 \
                 AND scope_kind = ?3 AND scope_id = ?4 AND deleted = 0 AND superseded_by IS NULL LIMIT 1",
                params![req.region.as_str(), content_hash, req.scope.kind.as_str(), req.scope.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, old_imp)) = existing {
            let new_imp = (old_imp as f32).max(req.importance);
            conn.execute(
                "UPDATE facts SET importance = ?1, access_count = access_count + 1, last_access_ms = ?2 WHERE id = ?3",
                params![new_imp as f64, now, id],
            )?;
            if let Some(rec) = get_record(&conn, id)? {
                let _ = self.ledger.append(
                    "memory.write",
                    &req.actor,
                    json!({ "region": req.region.as_str(), "content_hash": content_hash, "deduped": true }),
                );
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
            "INSERT INTO facts(region,text,importance,taint,tier,source,metadata,embedding,content_hash,ledger_seq,created_ms,last_access_ms,scope_kind,scope_id,embedding_bin) \
             VALUES(?1,?2,?3,?4,'warm',?5,?6,?7,?8,?9,?10,?10,?11,?12,?13)",
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
        })
    }

    /// Hybrid recall across the WHOLE brain (every scope): BM25 keyword + vector semantic,
    /// fused by RRF. `regions` empty means every region. Includes ALL provenance (even untrusted)
    /// - for transparency / audit / the Atlas. For model-facing recall use [`Memory::recall_scoped`]
    /// so the user/project/session rings are respected and one project can't read another's work.
    pub fn recall(&self, query: &str, regions: &[Region], k: usize) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, false, &ScopeCtx::any())
    }

    /// Whole-brain recall EXCLUDING untrusted-provenance memories. See [`Memory::recall_trusted_scoped`]
    /// for the ringed, model-facing variant.
    pub fn recall_trusted(&self, query: &str, regions: &[Region], k: usize) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, true, &ScopeCtx::any())
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
        self.recall_inner(query, regions, k, false, scope)
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
        self.recall_inner(query, regions, k, true, scope)
    }

    fn recall_inner(
        &self,
        query: &str,
        regions: &[Region],
        k: usize,
        trusted_only: bool,
        scope: &ScopeCtx,
    ) -> Result<Vec<Hit>> {
        let taint_clause = |prefix: &str| {
            if trusted_only {
                format!(" AND {prefix}taint = 'trusted'")
            } else {
                String::new()
            }
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
                 WHERE facts_fts MATCH ? AND f.deleted = 0 AND f.superseded_by IS NULL AND {region}{taint} AND {scope} \
                 ORDER BY bm25(facts_fts) LIMIT ?",
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

        // --- semantic arm: two-stage binary-quantized recall over ALL live in-scope vectors ---
        // Stage A (coarse): scan every live in-scope row's SMALL binary code and keep the COARSE_K
        // nearest by Hamming distance. This has NO salience cap - unlike the old top-5000-by-
        // importance scan, an old, low-importance, semantically-perfect memory is always eligible
        // (the completeness fix). The scope filter keeps the scan to the active rings, so per-recall
        // cost tracks the current project, not the whole brain. Stage B (exact): cosine-rerank just
        // those COARSE_K candidates on the full f32 embedding, for accurate ordering.
        let q_bin = quantize_binary(&q_emb);
        let (sem_scope_sql, sem_scope_binds) = scope_clause("", scope);
        let coarse_sql = format!(
            "SELECT id, embedding_bin FROM facts WHERE deleted = 0 AND superseded_by IS NULL AND {region}{taint} AND {scope}",
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
        coarse.sort_by_key(|(d, _)| *d);
        coarse.truncate(COARSE_K);
        // Stage B: exact cosine over the coarse candidates, then MMR to diversify - so near-duplicate
        // passages (e.g. overlapping document chunks) don't crowd out other relevant memories.
        let mut sims: Vec<(i64, f32, Vec<f32>)> = Vec::with_capacity(coarse.len());
        if !coarse.is_empty() {
            let ids: Vec<i64> = coarse.iter().map(|(_, id)| *id).collect();
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let exact_sql = format!("SELECT id, embedding FROM facts WHERE id IN ({placeholders})");
            let mut stmt = conn.prepare(&exact_sql)?;
            let binds: Vec<Value> = ids.iter().map(|i| Value::Integer(*i)).collect();
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

    /// Mark `old_id` as superseded by `by_id`: the old fact becomes history - kept (not
    /// deleted) and still in the ledger, but no longer surfaced by recall - while the new
    /// fact is the current truth. This is how a changed fact ("moved to Munich") evolves
    /// without erasing the past. Returns false if `old_id` wasn't a current, live memory.
    pub fn supersede(&self, old_id: i64, by_id: i64) -> Result<bool> {
        let n = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.execute(
                "UPDATE facts SET superseded_by = ?1 \
                 WHERE id = ?2 AND deleted = 0 AND superseded_by IS NULL",
                params![by_id, old_id],
            )?
        };
        if n > 0 {
            let _ = self.ledger.append(
                "memory.supersede",
                "core",
                json!({ "old": old_id, "by": by_id }),
            );
        }
        Ok(n > 0)
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
    pub fn recent_scoped(
        &self,
        region: Region,
        n: usize,
        scope: &ScopeCtx,
    ) -> Result<Vec<Record>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let (scope_sql, scope_binds) = scope_clause("", scope);
        let sql = format!(
            "SELECT {COLS} FROM facts WHERE region = ? AND deleted = 0 AND {scope} \
             ORDER BY created_ms DESC LIMIT ?",
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
        let demoted = {
            let conn = self.conn.lock().expect("memory mutex poisoned");
            conn.execute(
                "UPDATE facts SET tier = 'cold' \
                 WHERE tier = 'warm' AND deleted = 0 AND (?1 - last_access_ms) > ?2 AND importance < 0.7",
                params![now, cutoff],
            )? as i64
        };
        if demoted > 0 {
            self.ledger
                .append("memory.consolidate", "core", json!({ "demoted": demoted }))?;
        }
        Ok(demoted)
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
        Ok(Stats {
            total,
            by_region,
            by_tier,
        })
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
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
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
    // Created AFTER the ALTERs so the referenced columns exist on a migrated legacy brain too.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_facts_scope ON facts(scope_kind, scope_id, deleted)",
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
                WriteReq::new(Region::Semantic, "the api base url is set").scope(Scope::project("P")),
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
                WriteReq::new(Region::Semantic, format!("unrelated note number {i} about invoices"))
                    .importance(0.95),
            )
            .unwrap();
        }
        let target = m
            .remember(
                WriteReq::new(Region::Semantic, "the user preferences include a dark theme")
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
    fn reindex_binary_rebuilds_the_coarse_index_from_source_of_truth() {
        let (m, _d) = mem();
        for i in 0..5 {
            m.remember(WriteReq::new(Region::Semantic, format!("fact number {i}")))
                .unwrap();
        }
        m.remember(WriteReq::new(Region::Semantic, "the user prefers a dark theme"))
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
}
