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
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

use crate::embed::{cosine, from_bytes, to_bytes, Embedder};
use crate::region::Region;

/// How many candidates each search arm contributes before fusion.
const ARM_LIMIT: usize = 64;
/// Reciprocal Rank Fusion constant (standard default).
const RRF_K: f32 = 60.0;

const COLS: &str =
    "id,region,text,importance,taint,tier,source,metadata,content_hash,ledger_seq,created_ms,last_access_ms,access_count";

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
        Ok(mem)
    }

    /// Re-embed every stored memory when the active embedding model/space changes. Vectors
    /// from a different model live in an incomparable space, so without this a query under
    /// the new model would silently fail to match old memories. A no-op when the space is
    /// unchanged; on a genuine switch it re-embeds in one transaction and records it.
    fn migrate_embedding_space(&self) -> Result<()> {
        let current = format!("{}:{}", self.embedder.name(), self.embedder.dim());
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        let stored: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'embed_space'", [], |r| r.get(0))
            .optional()?;
        if stored.as_deref() == Some(current.as_str()) {
            return Ok(());
        }
        // Gather the live rows that need re-embedding (collect first so the statement is
        // dropped before we open the write transaction).
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn.prepare("SELECT id, text FROM facts WHERE deleted = 0")?;
            let mapped = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            let mut v = Vec::new();
            for row in mapped {
                v.push(row?);
            }
            v
        };
        // First-ever open with nothing stored: just stamp the space, nothing to migrate.
        if stored.is_none() && rows.is_empty() {
            conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES('embed_space', ?1)", params![current])?;
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
            let emb = to_bytes(&self.embedder.embed(text));
            tx.execute("UPDATE facts SET embedding = ?1 WHERE id = ?2", params![emb, id])?;
        }
        tx.execute("INSERT OR REPLACE INTO meta(key, value) VALUES('embed_space', ?1)", params![current])?;
        tx.commit()?;
        Ok(())
    }

    /// Store a memory: embed it, record it in the ledger, then persist it.
    pub fn remember(&self, req: WriteReq) -> Result<Record> {
        let embedding = self.embedder.embed(&req.text);
        let blob = to_bytes(&embedding);
        let content_hash = blake3::hash(req.text.as_bytes()).to_hex().to_string();
        let now = now_ms() as i64;
        let taint = taint_str(req.taint);

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
        let mut conn = self.conn.lock().expect("memory mutex poisoned");
        // One transaction so the row and its FTS index are all-or-nothing - a failure
        // can never leave the fact searchable-but-missing or present-but-unsearchable.
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO facts(region,text,importance,taint,tier,source,metadata,embedding,content_hash,ledger_seq,created_ms,last_access_ms) \
             VALUES(?1,?2,?3,?4,'warm',?5,?6,?7,?8,?9,?10,?10)",
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
        })
    }

    /// Hybrid recall: BM25 keyword search and vector semantic search, fused by RRF.
    /// `regions` empty means search the whole brain.
    /// Hybrid recall across `regions`, returning the top `k`. Includes ALL provenance
    /// (even memories written during an untrusted run) - for transparency / audit views.
    pub fn recall(&self, query: &str, regions: &[Region], k: usize) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, false)
    }

    /// Like [`Memory::recall`], but EXCLUDES untrusted-provenance memories - this is what a
    /// model should get as trusted context. Content read during a tainted run is stored
    /// with its provenance yet never silently re-surfaces here, closing the memory-poisoning
    /// vector (injected text can't become trusted memory and steer a later clean run).
    pub fn recall_trusted(&self, query: &str, regions: &[Region], k: usize) -> Result<Vec<Hit>> {
        self.recall_inner(query, regions, k, true)
    }

    fn recall_inner(
        &self,
        query: &str,
        regions: &[Region],
        k: usize,
        trusted_only: bool,
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
            let sql = format!(
                "SELECT facts_fts.rowid FROM facts_fts \
                 JOIN facts f ON f.id = facts_fts.rowid \
                 WHERE facts_fts MATCH ?1 AND f.deleted = 0 AND f.superseded_by IS NULL AND {region}{taint} \
                 ORDER BY bm25(facts_fts) LIMIT ?2",
                region = region_clause("f.", regions),
                taint = taint_clause("f.")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![match_q, ARM_LIMIT as i64], |r| r.get::<_, i64>(0))?;
            for id in rows {
                keyword.push(id?);
            }
        }

        // --- semantic arm (cosine over every candidate region row) ---
        let sem_sql = format!(
            "SELECT id, embedding FROM facts WHERE deleted = 0 AND superseded_by IS NULL AND {region}{taint}",
            region = region_clause("", regions),
            taint = taint_clause("")
        );
        let mut sims: Vec<(i64, f32)> = {
            let mut stmt = conn.prepare(&sem_sql)?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, blob) = row?;
                out.push((id, cosine(&q_emb, &from_bytes(&blob))));
            }
            out
        };
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sims.truncate(ARM_LIMIT);
        let semantic: Vec<i64> = sims.into_iter().map(|(id, _)| id).collect();

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
            let _ = self.ledger.append("memory.supersede", "core", json!({ "old": old_id, "by": by_id }));
        }
        Ok(n > 0)
    }

    /// IDs of current (live, non-superseded) memories in `region` whose text starts with
    /// `prefix` - used to find the prior singular fact a new one replaces.
    pub fn current_with_prefix(&self, region: Region, prefix: &str) -> Result<Vec<i64>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id FROM facts \
             WHERE region = ?1 AND deleted = 0 AND superseded_by IS NULL AND text LIKE ?2",
        )?;
        let like = format!("{prefix}%"); // prefixes are fixed RULE strings, no LIKE wildcards
        let rows = stmt.query_map(params![region.as_str(), like], |r| r.get::<_, i64>(0))?;
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

    /// The most recent `n` memories in a region, oldest-first - used to reload a
    /// conversation from episodic memory so the chat survives a refresh.
    pub fn recent(&self, region: Region, n: usize) -> Result<Vec<Record>> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let sql = format!(
            "SELECT {COLS} FROM facts WHERE region = ?1 AND deleted = 0 ORDER BY created_ms DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![region.as_str(), n as i64], map_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out.reverse();
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

    /// Counts for the Memory Atlas.
    pub fn stats(&self) -> Result<Stats> {
        let conn = self.conn.lock().expect("memory mutex poisoned");
        let total =
            conn.query_row("SELECT COUNT(*) FROM facts WHERE deleted = 0", [], |r| r.get(0))?;
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
            superseded_by INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_facts_region ON facts(region, deleted);
        CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(text, tokenize = 'unicode61');
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
    )?;
    // Add the supersession column to brains created before temporal validity (ignored if
    // it already exists).
    let _ = conn.execute("ALTER TABLE facts ADD COLUMN superseded_by INTEGER", []);
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
        m.remember(WriteReq::new(Region::Semantic, "Engram runs on a cheap VPS")).unwrap();
        let hits = m.recall("cheap VPS", &[Region::Semantic], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].record.text.contains("VPS"));
        assert!(hits[0].keyword_rank.is_some());
    }

    #[test]
    fn semantic_recall_finds_what_keyword_misses() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(Region::Identity, "the user preferences include a dark theme")).unwrap();
        m.remember(WriteReq::new(Region::Identity, "the weather in Berlin is cold today")).unwrap();

        // No shared whole-word tokens with the stored text ("preferred"/"theming"
        // are different tokens than "preferences"/"theme"), so keyword search alone
        // returns nothing - yet hybrid recall still surfaces the right memory.
        assert!(build_match("preferred theming")
            .map(|q| !q.contains("preferences"))
            .unwrap_or(true));
        let hits = m.recall("preferred theming", &[Region::Identity], 3).unwrap();
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
        m.remember(WriteReq::new(Region::Identity, "favourite language is Rust")).unwrap();
        m.remember(WriteReq::new(Region::Episodic, "favourite language is Rust")).unwrap();
        let hits = m.recall("favourite language", &[Region::Identity], 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.region, "identity");
    }

    #[test]
    fn recall_trusted_excludes_untrusted_provenance() {
        let (m, _d) = mem();
        m.remember(WriteReq::new(Region::Semantic, "Engram deploys to a cheap VPS")).unwrap();
        // Content read during a tainted run (e.g. scraped from an attacker page) inherits
        // Untrusted provenance.
        m.remember(
            WriteReq::new(Region::Semantic, "Engram deploys to a cheap VPS, per a web page")
                .taint(Taint::Untrusted),
        )
        .unwrap();

        // The transparency recall sees both…
        assert_eq!(m.recall("cheap VPS", &[Region::Semantic], 5).unwrap().len(), 2);
        // …but model-facing recall returns only the trusted one - injected memory can't
        // re-surface as trusted context and poison a clean run.
        let trusted = m.recall_trusted("cheap VPS", &[Region::Semantic], 5).unwrap();
        assert_eq!(trusted.len(), 1);
        assert_eq!(trusted[0].record.taint, "trusted");
    }

    #[test]
    fn switching_embedding_space_re_embeds_existing_memories() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let path = dir.path().join("brain.db");
        {
            let m = Memory::open(&path, Arc::new(TrigramHashEmbedder::new(256)), ledger.clone()).unwrap();
            m.remember(WriteReq::new(Region::Semantic, "Engram runs on a cheap VPS")).unwrap();
        }
        // Reopen under a different-dimension embedder - a new embedding space.
        let m = Memory::open(&path, Arc::new(TrigramHashEmbedder::new(128)), ledger.clone()).unwrap();
        // The migration ran and is recorded…
        assert!(ledger.read_all().unwrap().iter().any(|e| e.kind == "memory.reembed"));
        // …and recall still works against the re-embedded vectors.
        assert_eq!(m.recall("cheap VPS", &[Region::Semantic], 5).unwrap().len(), 1);
        // Reopening with the SAME embedder is a no-op - no second migration.
        let before = ledger.read_all().unwrap().iter().filter(|e| e.kind == "memory.reembed").count();
        let _again = Memory::open(&path, Arc::new(TrigramHashEmbedder::new(128)), ledger.clone()).unwrap();
        let after = ledger.read_all().unwrap().iter().filter(|e| e.kind == "memory.reembed").count();
        assert_eq!(before, after, "no migration when the embedding space is unchanged");
    }

    #[test]
    fn supersede_makes_the_old_fact_history_not_recalled() {
        let (m, _d) = mem();
        let berlin = m.remember(WriteReq::new(Region::Identity, "User lives Berlin")).unwrap();
        let munich = m.remember(WriteReq::new(Region::Identity, "User lives Munich")).unwrap();
        // Before supersession both surface.
        assert_eq!(m.recall("where the user lives", &[Region::Identity], 5).unwrap().len(), 2);

        assert!(m.supersede(berlin.id, munich.id).unwrap());

        // Only the current truth recalls - the stale fact can't be confidently wrong.
        let hits = m.recall("where the user lives", &[Region::Identity], 5).unwrap();
        assert!(hits.iter().all(|h| h.record.id != berlin.id), "superseded fact must not recall");
        assert!(hits.iter().any(|h| h.record.id == munich.id));
        // The prefix lookup now sees only the current value.
        assert_eq!(m.current_with_prefix(Region::Identity, "User lives ").unwrap(), vec![munich.id]);
        // Superseding a non-current id is a no-op.
        assert!(!m.supersede(berlin.id, munich.id).unwrap());
    }

    #[test]
    fn forget_then_restore() {
        let (m, _d) = mem();
        let rec = m.remember(WriteReq::new(Region::Semantic, "secret to forget")).unwrap();
        assert!(m.forget(rec.id, "user", "no longer relevant").unwrap());
        assert!(m.recall("secret", &[Region::Semantic], 5).unwrap().is_empty());
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
        let rec = m.remember(WriteReq::new(Region::Semantic, "tracked fact")).unwrap();
        assert!(rec.ledger_seq.is_some());
        assert!(ledger.verify().unwrap() >= 1);
    }

    #[test]
    fn consolidation_demotes_stale_low_importance() {
        let (m, _d) = mem();
        let rec = m.remember(WriteReq::new(Region::Episodic, "trivial chatter").importance(0.1)).unwrap();
        // Age the row past the warm window.
        {
            let conn = m.conn.lock().unwrap();
            conn.execute("UPDATE facts SET last_access_ms = 0 WHERE id = ?1", [rec.id]).unwrap();
        }
        let demoted = m.consolidate(Duration::from_secs(60)).unwrap();
        assert_eq!(demoted, 1);
        assert_eq!(m.get(rec.id).unwrap().unwrap().tier, "cold");
    }
}
