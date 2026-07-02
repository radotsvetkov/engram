# Engram Memory Architecture — Scoped, Scalable, Verifiable

**Status:** the original design/build plan below was **partly superseded during implementation**. **Branch:** `feat/scoped-scalable-memory`.

> **What actually shipped — read [§10b](#10b-implementation-status-built-2026-07-01-autonomous-run) first.** The scope model, union-of-rings recall, layered consciousness, the completeness fix, the document corpus, MMR rerank, and the budget packer all shipped. But the vector-store choice changed: **`sqlite-vec` was rejected in favour of pure-Rust binary quantization over the existing SQLite** (see §10b for why), so the `vec_items` vec0 virtual table and the separate `documents`/`chunks`/`parents` tables in §§3–4/6–7 below **were not built** — document chunks land as scoped memory facts (`crates/engramd/src/corpus.rs`). The `engramd reindex` CLI, `/v1/metrics`, and `/v1/documents/*` referenced below **do not exist**; the shipped surfaces are `POST /v1/memory/reindex` and the on-open `backfill_binary`. Treat §§3–9 as the superseded design rationale, not the as-built system.

This unifies two problems into one memory system:

1. **Isolation** — cross-project memory bleed (a chat in project B recalls project A's work).
2. **Scale** — many projects, thousands of files, and huge candidate sets feeding a bounded context window, without recall going O(n) or silently dropping relevant memories.

It extends the current brain rather than replacing it: one SQLite WAL file, one signed ledger, one embedding space, offline-by-default, 0 MB at idle. Line numbers are against the working tree as of this writing.

> **Read this first — what the adversarial review corrected.** An earlier draft sold `sqlite-vec` as "a real ANN index that replaces the O(n) scan." That is false: `sqlite-vec` (vec0) is **brute-force KNN only** (DiskANN is unreleased roadmap), and its partition-key `IN` filter is unshipped. The real scale levers here are **(1) scope partitioning** (shrink the scanned set to the active project), **(2) binary quantization** (a 32× cheaper coarse Hamming pass), and **(3) a true ANN — IVF or `hnsw_rs` — only for a single scope that grows past ~100–200k live vectors.** Everything below reflects the corrected story.

---

## 1. North star & invariants

**End state:** a single static musl binary (<15 MB) holds the entire brain — memories, document chunks, hierarchical summaries, and the signed ledger — in one `brain.db`. Recall is **scope-partitioned** (user / project / session rings), **complete** (no salience cap silently hiding old rows), and **bounded** (a token-budget packer fits a reranked, diversified, hierarchy-aware set into the model window). Files are a first-class, project-scoped, incrementally-indexed corpus riding the same index and the same union-of-rings recall.

Invariants that must never break:

| # | Invariant | Enforcement |
|---|---|---|
| I1 | One signed append-only ledger; every write signed *before* it lands | Every new write path calls `Ledger::append` before its txn commits — same discipline as `remember` ([store.rs:260](../crates/engram-memory/src/store.rs:260)). New entry kinds are opaque to `verify()`. |
| I2 | Taint sticky & monotonic; untrusted never enters trusted recall or user-global identity | `taint` is a **filterable column inside the vector index**, applied in the **coarse** scan (not post-hoc). Rollup taint = join over children. Document chunks default untrusted. |
| I3 | Verifiable consciousness — each line verbatim from a real trusted memory, signed | Consciousness splits into global + per-project blocks; distill still draws only trusted rows; each line still traces to a fact id. |
| I4 | Offline-capable by default | Default embedder stays `TrigramHashEmbedder`; `StaticEmbedder` (model2vec, **already in tree**) is the offline real-semantics upgrade; gateway is opt-in and off the recall hot path. Rerank/summarize default to pure-Rust extractive. |
| I5 | $5-VPS: 0 MB idle, <50 ms cold start, single file | No sidecar, no resident index, nothing warm survives sleep. Heavy work runs only on active/scheduled wakes. |
| I6 | New project clean by construction; backward compatible | Scope columns `DEFAULT 'user'` make every existing row global; a new project's ring is empty by definition. |

---

## 2. The scope model (the isolation fix)

**Scope is orthogonal to region.** `region` = *what kind* of memory (episodic/semantic/identity/procedural). Scope = *which world* (user/project/session). They never collapse into one enum.

**Recall is a union of rings, not a filter.** With project `P` and session `S` active, recall returns `user ∪ project:P ∪ session:S`, specificity-boosted (session > project > user) after fusion. No project active → user-global only. This is the one model that delivers "general facts about me follow me everywhere, project work stays put." A hard project filter would also hide your global preferences inside the project — which we explicitly don't want.

**Write-time router** classifies each capture into the narrowest applicable scope (`engramd/src/scope.rs`, new):

- Identity facts about *you* → **user** (always global).
- Ephemeral markers ("for now", "never mind") → **session**.
- No project active → user (semantic) or session (episodic).
- "i prefer / i always / in general" → **user** even inside a project.
- Otherwise, with a project active → **project**.

Default narrowest; promote on explicit signal. A wrong "global" contaminates every project; a wrong "project" is harmless. **A global write from a tainted/web run is staged behind the autonomy approval gate** — only trusted may enter user-global identity (I2).

---

## 3. Data model

> **Superseded in part — see §10b.** The `facts` scope columns shipped. The `vec_items` vec0 virtual table (§3.2) and the `documents`/`chunks`/`parents` tables (§3.3) were **not** built: sqlite-vec was dropped, the binary-quantized vector lives inline on `facts` (`embedding_bin`), and document chunks are stored as scoped memory facts rather than in a separate corpus schema (`crates/engramd/src/corpus.rs`). The DDL below is retained as the design that motivated the shipped simpler form.

One file `<home>/brain.db` (SQLite WAL); one ledger `<home>/ledger.jsonl` (unchanged). All DDL lands in `init_schema` ([store.rs:583](../crates/engram-memory/src/store.rs:583)) as `IF NOT EXISTS` + idempotent `ALTER TABLE ADD COLUMN` (the pattern already used for `superseded_by` at store.rs:611).

### 3.1 `facts` — extended, not replaced

```sql
-- scope lattice
ALTER TABLE facts ADD COLUMN scope_kind   TEXT    NOT NULL DEFAULT 'user';  -- user|project|session
ALTER TABLE facts ADD COLUMN scope_id     TEXT    NOT NULL DEFAULT '';      -- ''=user-global
ALTER TABLE facts ADD COLUMN scope_bucket INTEGER NOT NULL DEFAULT 0;       -- derived partition key (§4.2)
-- hierarchical rollups
ALTER TABLE facts ADD COLUMN kind          TEXT    NOT NULL DEFAULT 'leaf';  -- leaf|rollup
ALTER TABLE facts ADD COLUMN rollup_level  INTEGER NOT NULL DEFAULT 0;       -- 0 leaf, 1..n summaries
ALTER TABLE facts ADD COLUMN child_ids     TEXT;                             -- JSON [i64]
-- resumable re-embed bookkeeping
ALTER TABLE facts ADD COLUMN embed_dirty   INTEGER NOT NULL DEFAULT 0;       -- 1 = vector stale

CREATE INDEX IF NOT EXISTS idx_facts_scope  ON facts(scope_kind, scope_id, deleted);
CREATE INDEX IF NOT EXISTS idx_facts_rollup ON facts(kind, rollup_level, scope_kind, scope_id);
```

Also update `COLS` ([store.rs:34](../crates/engram-memory/src/store.rs:34)), the `Record` struct, and `map_record` ([store.rs:632](../crates/engram-memory/src/store.rs:632)) to surface the new columns — the read path needed for scope-aware supersede and the UI scope badge.

**Decision — drop inline `embedding BLOB` after backfill.** The vector is index data, not content; it lived inline only because there was no index. Once `vec_items` is populated, `remember` stops writing `facts.embedding` and recall never reads it. Keep the column nullable for one release for rollback, then remove. (See §7 on the transient storage cost during this window.)

### 3.2 `vec_items` — one vector table over facts **and** chunks

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS vec_items USING vec0(
    item_id       INTEGER PRIMARY KEY,   -- (kind_bit<<62 | rowid); fact & chunk ids never collide
    scope_bucket  INTEGER,                -- METADATA (not partition key — see §4.2), supports =/IN filter
    scope_id      TEXT,                   -- exact project/session match inside a bucket
    kind          INTEGER,                -- 0=fact, 1=chunk
    region        INTEGER,                -- episodic0 semantic1 identity2 procedural3 rollup4
    taint         INTEGER,                -- 1=trusted 0=untrusted (I2, filtered in coarse pass)
    live          INTEGER,                -- 1 if not deleted and not superseded
    embedding     float[256] distance_metric=cosine,   -- full precision (dim parametric, §7)
    embedding_bin bit[256]                               -- binary-quantized coarse companion
);
```

One shared table (not separate `vec_facts`/`vec_chunks`) so union-of-rings recall including files is a single KNN that returns memories and document passages together; `kind` lets consciousness distill ask for facts-only.

### 3.3 Documents / chunks / parents (the file corpus)

```sql
CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY,
    scope_kind TEXT NOT NULL DEFAULT 'user', scope_id TEXT, scope_bucket INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL, mime TEXT, byte_len INTEGER NOT NULL,
    file_hash TEXT NOT NULL,                       -- blake3 of RAW bytes (dedup key)
    stored_ref TEXT NOT NULL,
    taint TEXT NOT NULL DEFAULT 'untrusted',       -- files are untrusted-provenance (I2)
    extract_hash TEXT, chunk_strategy TEXT NOT NULL DEFAULT 'recursive-512',
    embed_space TEXT,
    status TEXT NOT NULL DEFAULT 'pending',         -- pending|extracting|chunking|embedding|indexed|failed|skipped
    n_chunks INTEGER NOT NULL DEFAULT 0, error TEXT,
    ledger_seq INTEGER, created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_scope_hash
    ON documents(scope_kind, scope_id, file_hash) WHERE deleted = 0;   -- re-upload same file to same scope = NOOP

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY, document_id INTEGER NOT NULL,
    scope_kind TEXT NOT NULL DEFAULT 'user', scope_id TEXT, scope_bucket INTEGER NOT NULL DEFAULT 0,
    taint TEXT NOT NULL DEFAULT 'untrusted',        -- denormalized from parent, sticky (I2)
    ordinal INTEGER NOT NULL, parent_ordinal INTEGER NOT NULL,
    text TEXT NOT NULL,                             -- ~128-200 tok child (embed/retrieve unit)
    char_start INTEGER NOT NULL, char_end INTEGER NOT NULL, section TEXT,  -- citation
    content_hash TEXT NOT NULL,                     -- incremental-skip key
    ledger_seq INTEGER, created_ms INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(document_id, content_hash);

CREATE TABLE IF NOT EXISTS parents (        -- ~500-tok window returned on a child hit (small-to-big)
    document_id INTEGER NOT NULL, parent_ordinal INTEGER NOT NULL,
    text TEXT NOT NULL, char_start INTEGER NOT NULL, char_end INTEGER NOT NULL,
    PRIMARY KEY (document_id, parent_ordinal)
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(text, tokenize = 'unicode61');
```

### 3.4 Ledger additions

New signed `kind` strings, each appended before the corresponding landing: `document.ingest`, `document.chunks_indexed`, `document.delete`, `document.trust`, `document.reembed`, `memory.rollup`, `memory.reembed_batch`, `scope.promote`. No ledger schema change — `verify()` ([ledger.rs:207](../crates/engram-core/src/ledger.rs:207)) replays them as opaque kinds.

---

## 4. The unified retrieval pipeline

```
query + active {project P, session S}
  │
  ├─ (1) SCOPE RESOLVE: buckets = [user] (+ project:P) (+ session:S); regions = requested or ALL(±rollup)
  ├─ (2) EMBED QUERY ONCE: q_f32; q_bin = sign-bits(q_f32); gateway path LRU-cached, never blocks
  ├─ (3) CANDIDATE GEN (scope-filtered, COMPLETE — no salience cap):
  │      • keyword arm: facts_fts ∪ chunks_fts MATCH, scoped+tainted, BM25 LIMIT 64
  │      • semantic arm, two-stage, per bucket:
  │          A. coarse Hamming KNN over embedding_bin, k=256,
  │             WHERE scope_bucket=<b> AND live=1 AND (taint=1 if trusted) [AND region/kind]   ← filter in COARSE pass
  │          B. exact cosine rerank of those over embedding(f32), keep top 64
  │        (one A/B per active bucket — see §4.2 — then merge)
  ├─ (4) FUSION: RRF (RRF_K=60) over keyword ∪ semantic; + specificity boost post-RRF
  ├─ (5) RERANK + MMR (rerank.rs, pure-Rust): feature-linear score → MMR λ=0.7; ~150 → diverse ~20-40
  ├─ (6) HIERARCHICAL EXPAND: rollup hit → abstract or drill child_ids; chunk hit → its parents[] window
  ├─ (7) BUDGET PACK (budget.rs): tiered knapsack under ctx − reserved_out − 8%; overflow → JIT summarize
  └─ (8) PROMPT → provider   (order: system/consciousness top · best-evidence+query at bottom)
```

Per-hit access bookkeeping (the `UPDATE last_access_ms` currently inside the recall lock at store.rs:416) moves **off the read path** into a batched async writer, so recall becomes a pure read.

### 4.1 The two scale levers (honest version)

1. **Scope partitioning** — a project recall only scans that project's vectors, not the whole brain. This is what makes "10⁶ total but per-query cost tracks the active project" true. It is the dominant lever.
2. **Binary quantization** — the coarse pass compares `bit[256]` vectors with SIMD popcount (Hamming), ~32× cheaper per vector than f32 cosine; the exact f32 rerank runs only over the ~256 coarse survivors. This is still a *full scan of the in-scope set*, just cheap.

`sqlite-vec` gives us vector storage, SIMD brute-force KNN, and metadata filtering — **not** an ANN. It is the right in-file, zero-idle, single-binary choice ([ADR-0001](ADR-0001-architecture.md)), but it does not itself defeat O(n); the two levers above do.

3. **True ANN, only where needed** — when a *single* scope grows past ~100–200k live vectors, its coarse scan exceeds ~10 ms. Then we build a per-scope IVF centroid pre-filter (√S k-means cells computed on a Trusted consolidation wake, stored in a small `vec_centroids` table) — or, as an alternative, a pure-Rust `hnsw_rs` index over that scope's subset. This drops the scan ~10–30× at ~0.97 recall. Brute-force stays the default; the ANN recall tax is paid only where it demonstrably hurts.

### 4.2 `scope_bucket` and why it's metadata, not a partition key

```
scope_bucket = 0                            for user  (all global share bucket 0)
scope_bucket = HI + hash32(project)         for project
scope_bucket = 1 + (hash32(session) % 4096) for session (coarse, avoids over-sharding)
```

`sqlite-vec` partition keys do **not** support `IN` (unshipped). So `scope_bucket` is a **metadata column**, and union-of-rings recall issues **one `=` KNN per active bucket** (user, project, session → up to 3 queries) and merges the results — rather than a single multi-bucket `IN`. This keeps per-query cost proportional to the *active scope's* size (each `=` query is filtered to one bucket in the coarse pass), at the cost of ≤3 KNN calls per recall. When partition-key `IN` ships upstream, this collapses back to one call.

### 4.3 Latency & big-O (S = live vectors in active scope, d = dim)

| Stage | Cost | S=10⁴ (one project), d=256 |
|---|---|---|
| Scope resolve + embed query | O(d) | <1 ms (cached) |
| Keyword (FTS5 BM25) | O(log N) | ~0.2 ms |
| Coarse Hamming per bucket (SIMD popcnt) | O(S·d/8) | ~0.3 ms (320 KB) |
| Exact f32 rerank of ≤256 | O(256·d) | ~0.1 ms |
| RRF + linear rerank + MMR | O(cand²·d), cand≤150 | <0.5 ms |
| Budget pack | O(items) | <0.3 ms |

**Targets (scoped recall):** p50 ≤ 8 ms, p99 ≤ 25 ms at d=256. With an RO reader pool, p99 is the max of in-flight recalls, not the sum. **Honest slow path:** a *global* (no-project) recall over a true 10⁶ live set is a 32 MB Hamming scan; warm that's ~30–60 ms, but on a 0-MB-idle core the **first** global recall after each idle self-exit is served cold from disk (100s of ms on a slow $5-VPS disk, re-cooling every idle cycle). Mitigation: scope partitioning makes unscoped global recall rare; the `<50 ms` cold-start budget is exec-to-first-byte, not recall-after-cold-cache — they are not the same number.

---

## 5. Consciousness (working memory), layered

The always-loaded block splits ([conscious.rs:130](../crates/engramd/src/conscious.rs:130)):

- **Global block** (`consciousness.json`) — distilled from user-global identity + semantic memory. Loaded every run.
- **Per-project block** (`consciousness/<pid>.json`, ≤4 lines) — distilled from that project's semantic memory. Loaded only when that project is active.

`prompt_block(project)` concatenates `[global] + [project]` under one budget. Every line is still verbatim from a real, trusted, signed memory (I3). Structurally, a project note can no longer pollute the block shown in every project.

---

## 6. Robustness & reliability

- **Completeness (the headline fix):** delete the `SEM_SCAN_CAP=5000` salience cutoff ([store.rs:373](../crates/engram-memory/src/store.rs:373)) — the coarse pass considers *every live in-scope vector*, so an old, low-importance, semantically-perfect memory is always eligible. Leaves are never deleted for cost (demote-don't-delete); rollups are an *additional* entry point, never a replacement; prompt overflow is summarized, not dropped.
- **Crash-safety:** ledger-first everywhere — a crash between append and commit leaves an unreferenced ledger entry (harmless, replay-detectable), never a landed-but-unsigned row. Ingest is a resumable state machine keyed on `documents.status`, landing per sub-batch. Re-embed is cursor-checkpointed (`meta.embed_migration_cursor`).
- **Write serialization is intentional:** all writes serialize on the ledger tip lock ([ledger.rs:155](../crates/engram-core/src/ledger.rs:155)) — required for the signed hash-chain and *not* parallelizable. Reads parallelize via the RO pool; writes are batched coarsely (one ledger entry per ingest sub-batch) to keep the fsync/flush cost bounded. (Confirm `flush()` durability semantics; if fdatasync is needed for durability, batch harder.)
- **Index is derived & rebuildable:** the binary-quant index is derived from `facts` (the source of truth). *(As built: `POST /v1/memory/reindex` → `reindex_binary` drops and rebuilds it; there is no `engramd reindex` CLI subcommand.)* Full recovery from a corrupt/missing index with zero data loss. `engramd verify` replays the ledger against the file offline, independent of the index.
- **sqlite-vec is pre-1.0 (alpha):** on-disk vec0 format may change across versions. Tolerable *only because* `vec_items` is derived: treat `reindex` as a first-class migration step run on every sqlite-vec version bump; keep the ledger/verify path independent of the vec0 format (it is). Do not treat `vec_items` as durable state.
- **Static linking (security + musl):** integrate sqlite-vec via **static link + `sqlite3_auto_extension`**, with `conn.load_extension()` left **disabled**. Add a CI musl target that proves the sqlite-vec C translation unit links statically. Runtime `.so` loading would violate the single-binary invariant and open an extension-load attack surface — it is rejected.
- **Embedding-space migration at scale:** `Memory::open` only *detects* a space change and stamps `embed_migration_pending`; a background job on the next active wake re-embeds in batches of ~256 (one batched gateway call per batch, not 256 serial), one ledger entry per batch, cursor-advanced, resumable. Boot stays <50 ms. Not-yet-migrated rows are `embed_dirty=1` and searched via their old-space vector (degraded, not dark). This replaces today's single mega-transaction inside `open` ([store.rs:160](../crates/engram-memory/src/store.rs:160)).
- **Degradation:** gateway down → recall uses a local fallback vector, warms async, never blocks; a write-time gateway embed failure leaves the row `embed_dirty` and retried — it must **not** silently store a trigram vector into the transformer space (fixes the fallback at [embedder.rs](../crates/engramd/src/embedder.rs)).
- **Observability:** *(designed, not built — there is no `/v1/metrics` route)* per-recall coarse-scan size, per-arm candidate counts, rerank/MMR drops, budget utilization + overflow-summarize count, ingest queue depth, `embed_dirty` count, migration cursor, `verify` tip hash.

---

## 7. Scale math (small VPS, ~1–2 GB RAM; dim=256 unless noted)

| Corpus | Disk `brain.db` | Scoped recall | Notes |
|---|---|---|---|
| 10 projects × 10⁴ facts = 10⁵ | ~185 MB (f32 100 MB, bin 3.2 MB, text 50 MB, FTS 30 MB) | p50 ~3 ms, p99 ~12 ms | fits trivially; 0 MB idle |
| 100 projects × 10⁴ = 10⁶ | ~1.9 GB steady state | still ~3–12 ms per **project** recall | per-query cost tracks the active ring, not N |
| 10⁴ files (~20 chunks) = 2×10⁵ chunks | +~630 MB | sub-ms coarse per project corpus | streaming ingest, constant RAM |

**Honesty caveats the review forced in:**
- **Transient ~2× vector storage during migration.** While the inline `facts.embedding` is retained "for one release" *and* `vec_items` f32 is populated, the 10⁶ case is ~2.9 GB, not 1.9 GB. The 1.9 GB figure is the post-drop steady state. The inline-embedding drop must complete before claiming steady-state disk.
- **Cold global recall ≠ warm p99.** See §4.3 — first unscoped global recall after idle is cold-from-disk.
- **Dim is not fixed at 256.** `StaticEmbedder.dim()` is read from the model matrix ([static_embed.rs:81](../crates/engram-memory/src/static_embed.rs:81)); a 512/768-dim model2vec export triples every storage/scan number. Pin a recommended 256-dim export, validate dim at load, and keep the scale math parametric in dim.

**Named limits & escape hatches:** single scope > ~200k live → IVF/`hnsw_rs` per-scope index; whole-file f32 storage dominates at 10⁶ → binary-quantized-only mode (drop f32, 32× smaller, ~0.97 recall) as a config toggle; dim=1536 gateway → Matryoshka truncation to 512 or default to 256 StaticEmbedder.

---

## 8. Constraint-respecting choices

| Pick | Choice | Verdict |
|---|---|---|
| Vector store | `sqlite-vec` vec0, **statically linked** into bundled SQLite | Fits single-file/zero-idle/musl. **Brute-force KNN + metadata filter, not ANN** — see §4.1. |
| Embedder default | `TrigramHashEmbedder` ([embed.rs:51](../crates/engram-memory/src/embed.rs:51)) | Fits — offline, zero-dep |
| Embedder upgrade | `StaticEmbedder` (model2vec) — **already in tree**, wired via `config.embed.kind` ([config.rs:343](../crates/engramd/src/config.rs:343)), selected at [main.rs:730](../crates/engramd/src/main.rs:730) | Fits — pure-Rust matrix lookup, ~30 MB deploy-time data dir, no ONNX |
| Embedder max | Gateway (1536-dim), **off the recall hot path** | Fits as opt-in max |
| Chunker | recursive/structure-aware @512 parent / ~128 child, 10–15% overlap, pure-Rust | Fits — no LLM call |
| Rerank | feature-linear (fuse+sim+kw+importance+recency+taint+scope), pure-Rust | Fits — no cross-encoder |
| Diversity | MMR λ=0.7 on in-hand embeddings | Fits |
| Summarize | extractive (top sentences), pure-Rust | Fits; gateway map-reduce = opt-in (Trusted wake only) |
| Token count | `approx_tokens` (~4 chars/tok, [provider.rs:62](../crates/engram-gateway/src/provider.rs:62)) | Fits |

**Rejected:** bundled ONNX/`ort`; external vector DB (Pinecone/Qdrant/Lance) or FAISS/hnswlib `.so`; resident index server / warm-model daemon / background worker thread; network-default embedder; any index that skips the ledger or drops taint; **selling sqlite-vec as ANN**; keeping `SEM_SCAN_CAP` (the silent-completeness-loss bug); hard-deleting cold memories (breaks completeness — demote, don't erase); separate `vec_facts`/`vec_chunks` tables (would make recall two queries).

---

## 9. Migration (zero data loss, staged)

1. **Additive schema** on next `Memory::open` — existing rows become `scope_kind='user'`, i.e. globally recallable exactly as today (I6). Old `brain.db` opens unchanged.
2. **Backfill** the vector index from existing `facts.embedding` in batches (lazy on first wake, or `POST /v1/memory/reindex` — *not* an `engramd reindex` CLI, which does not exist), one ledger entry per batch, idempotent/resumable. The old brute-force `recall_inner` stays live and correct until the index is fully populated; a feature flag flips recall to the new path.
3. **Embedder upgrade** (`ENGRAM_EMBED=static`) triggers the reworked batched/checkpointed re-embed. Trigram → static → gateway are monotonic quality upgrades, each resumable; `embed_dirty` rows searchable via old vector meanwhile.
4. **Documents** tables start empty; existing uploads re-ingested lazily on next reference. The inline 8000-char attachment path ([converse.rs:154](../crates/engramd/src/converse.rs:154)) stays as the fallback for un-ingested files.
5. **Rollout flags** `ENGRAM_ANN`, `ENGRAM_CORPUS`, `ENGRAM_ROLLUPS`, `ENGRAM_BUDGET` default off, flipped per phase; every phase independently revertible.

---

## 10. Phased build plan

> **This is the original plan; §10b records what actually shipped and what was deferred.** Where a phase names `sqlite-vec`/`vec_items`, the `documents`/`chunks`/`parents` DDL, an `engramd reindex` CLI, or `/v1/documents/*` routes, those were **not** built as written — see §10b. The shipped equivalents are pure-Rust binary quantization inline on `facts`, corpus chunks as scoped memory facts, and `POST /v1/memory/reindex`.

Each phase: goal · files · proving test. Phases 0–2 are the isolation base (shippable alone, stops the bleed); 3–5 the retrieval core; 6–7 documents; 8–9 orchestration; 10 hardening.

**Phase 0 — Schema + scope columns.** `store.rs` `init_schema`, `COLS`, `Record`/`map_record`, `remember`, `WriteReq`. *Test:* a pre-existing brain reads back every row as `scope_kind='user'`; a scoped remember round-trips.

**Phase 1 — Write-time scope router.** New `engramd/src/scope.rs`; thread active `project_id`/`session_id` from `workspace.rs` (Project 16 / Session 43) into the capture sites in `converse.rs` + the run path. *Test:* project capture lands `project:P`; a tainted-run global write is staged; only trusted writes user-global identity.

**Phase 2 — Scoped recall (still brute-force) + layered consciousness.** Union-of-rings filter in the existing `recall_inner` (both arms) + specificity boost; scope-aware `supersede`/`current_with_prefix`; split `distill`/`prompt_block` by scope; thread scope through `ToolCtx` (**~20 construction sites incl. every subagent clone in `agent.rs`**) and the WASM host's `RunCtx`+`HostState`; keep default `recent()` wrappers so the Atlas/`memory_graph` callers see all scopes. *Test:* a fact in project A is invisible when B is active, visible under A or globally; a fresh project's consciousness block is empty while the global block is stable. **← the bleed is fixed here.**

**Phase 3 — sqlite-vec + `vec_items`.** Static-link + `sqlite3_auto_extension`; create `vec_items`; `remember`/`forget`/supersede maintain it; `engramd reindex` backfill; CI musl link check. *Test:* remember lands a vec row with correct scope_bucket/taint/live; `reindex` reproduces vec rows byte-identically; forget flips `live=0`.

**Phase 4 — Two-stage KNN recall (the completeness fix).** `quantize_binary` in `embed.rs`; per-bucket coarse-Hamming → f32-rerank in `recall_inner` with filters in the coarse pass; remove `SEM_SCAN_CAP`; behind `ENGRAM_ANN`. *Test:* a low-importance old row outside today's top-5000 IS recalled by paraphrase; latency flat over a 10⁵-row seed; untrusted row excluded from `recall_trusted` at the index level.

**Phase 5 — Reader pool + deferred access writes + reworked migration.** `Mutex<Connection>` → RO reader pool + one writer; batched async `last_access` writer; `migrate_embedding_space` → detect-in-`open` + background cursor-checkpointed batches. *Test:* N concurrent recalls run in parallel; an embedder swap boots <50 ms and migrates in resumable batches; kill mid-migration resumes from cursor.

**Phase 6 — Document ingest pipeline.** New `engramd/src/corpus.rs` (extract → chunk → embed → store state machine, content-hash incremental, `ingest_queue` drain); documents/chunks/parents/chunks_fts DDL + store methods; `extract_document_text` ([main.rs:3854](../crates/engramd/src/main.rs:3854)) → streaming; `upload_handler` → `corpus::ingest`; routes `/v1/documents/*`. *Test:* upload a 50-page PDF → chunks+parents indexed, retrievable next turn; re-upload identical file = NOOP; edit one page → only changed chunks re-embed; chunks carry `taint='untrusted'`.

**Phase 7 — Corpus recall integration.** `chunks_fts` into the keyword arm; parent-expand chunk hits; `Hit` gains citation fields; `attachments_context` prefers the indexed corpus over 8000-char re-truncation. *Test:* a query is answered from a doc indexed 3 turns earlier, with file+section citation; project-scoped query never returns another project's file; the parent window (not the raw child) reaches the model.

**Phase 8 — Rerank + MMR + hierarchical rollups.** New `engram-memory/src/rerank.rs`; `Region::Rollup`; widen fanout; `consolidate` → `build_rollups` (extractive, ledger-first, taint = join over children, demote-don't-delete); `expand_rollup`; `recall_orchestrated`. *Test:* two paraphrases → MMR keeps one; a rare-token exact match ranks above a semantic near-miss; 300 episodic rows in a project produce a level-1 rollup that recalls as one entry; a rollup over any untrusted leaf is itself untrusted.

**Phase 9 — Context-budget manager.** New `engramd/src/budget.rs` (`pack`, tiers, extractive `compress`); replace the naive `recalled.join` assembly in `converse.rs` (236–256) and the `parts.join` at [main.rs:2509](../crates/engramd/src/main.rs:2509). *Test:* a flood of recalled rows never starves the system prompt (tier floor holds); overflow is summarized + re-admitted, not dropped; the prompt fits `ctx − reserved − 8%`; best evidence sits adjacent to the query.

**Phase 10 — Repair, observability, escape hatches.** `engramd reindex` full rebuild; `/v1/metrics`; optional per-scope IVF/`hnsw_rs` for scopes > 200k; binary-only mode toggle. *Test:* delete `vec_items`, `reindex` rebuilds it, recall identical; `engramd verify` passes after all new write kinds; a synthetic 300k-vector scope auto-escalates to the ANN path at ~0.97 recall.

---

## 10b. Implementation status (built 2026-07-01, autonomous run)

Delivered and fully tested (whole workspace green; memory 23, engramd 56, core 25, plus the rest):

| Phase | What shipped | Key files |
|---|---|---|
| P0 | `Scope`/`ScopeKind`/`ScopeCtx` + `facts.scope_kind/scope_id` + read/write path | [scope.rs](../crates/engram-core/src/scope.rs), [store.rs](../crates/engram-memory/src/store.rs) |
| P1 | Union-of-rings recall (`recall_scoped`/`recall_trusted_scoped`/`recent_scoped`/`current_with_prefix_scoped`) + specificity boost; bare `recall()` stays whole-brain | [store.rs](../crates/engram-memory/src/store.rs) |
| P2 | Write-time router + `scope_for_session`; scope threaded through converse, agentic runs, flywheel, ribbon, identity, the `memory_recall`/`memory_remember` tools, and the WASM skill host | [scope.rs](../crates/engramd/src/scope.rs), [converse.rs](../crates/engramd/src/converse.rs), [main.rs](../crates/engramd/src/main.rs), [tool.rs](../crates/engram-agent/src/tool.rs), [host.rs](../crates/engram-skills/src/host.rs) |
| P3 | Layered consciousness — global block is user-only; per-project block via `conscious::project_block` | [conscious.rs](../crates/engramd/src/conscious.rs) |
| P4 | **Pure-Rust** binary-quantization two-stage recall (`quantize_binary`/`hamming`, `embedding_bin` + backfill); removed `SEM_SCAN_CAP` → completeness fix | [embed.rs](../crates/engram-memory/src/embed.rs), [store.rs](../crates/engram-memory/src/store.rs) |
| P6/P7 | Document corpus as scoped chunks (`corpus::chunk_text`/`ingest_document`), wired into upload, project-isolated retrieval with `document:<name>#<i>` provenance | [corpus.rs](../crates/engramd/src/corpus.rs) |
| P8 | MMR diversity rerank in the semantic arm | [rerank.rs](../crates/engram-memory/src/rerank.rs) |
| P9 | Context-budget packer (tiered, essentials-survive) wired into the run's prompt assembly | [budget.rs](../crates/engramd/src/budget.rs) |
| P10 | `reindex_binary` index repair + `POST /v1/memory/reindex` | [store.rs](../crates/engram-memory/src/store.rs) |

**Deliberately deferred (engineering call, not omission):**
- **`sqlite-vec` → replaced with pure-Rust binary quantization.** The verification proved sqlite-vec is brute-force (not ANN), alpha with on-disk-format risk, needs static-link gymnastics, and lacks partition-`IN`. The two real levers (scope partitioning + binary quantization) are delivered in pure Rust over the existing SQLite with zero new C deps — a better fit for the tiny-binary/offline/$5-VPS ethos.
- **P5 reader pool / off-boot resumable embedding migration.** A RO connection pool trades real concurrency-bug surface for negligible benefit at single-user scale (recalls are sub-millisecond; writes must serialise on the ledger regardless). Resumable off-boot re-embedding only matters at 10⁵⁺ rows on an embedder switch, which no current install hits. `backfill_binary` (on open) + `reindex_binary` (repair) cover the common cases. Revisit if a deployment genuinely reaches that scale.
- **P8 hierarchical rollups (RAPTOR-style summary nodes).** MMR delivers the near-term diversity win; recursive cluster-summarisation is a larger feature worth its own pass once real episodic volume warrants it.

## 11. Two facts that de-risk the build

1. `StaticEmbedder` (model2vec, pure-Rust, offline real-semantics) and its config wiring **already exist** — the embedder-quality upgrade is mostly promoting it to the recommended tier + reworking migration, not net-new code.
2. The ledger already exposes `append(kind, actor, payload)` treating new kinds as opaque — every new write path joins the signed chain with zero ledger changes and `verify` keeps working.
