# Engram memory benchmarks

Real, committed, re-runnable numbers for the memory-upgrade plan's headline claims
(`docs/MEMORY-UPGRADE-PLAN.md`). Every number below comes from running the code in this repo — no
number is asserted or hand-picked; the methodology and the diagnostic to reproduce it are both
described. Where a result was surprising, it's reported as found, not smoothed over.

This is an *internal* benchmark suite: it measures Engram's own hybrid retrieval against isolated
single-signal baselines (keyword-only, semantic-only) built from the exact same stored index, and
measures Engram's own scaling behavior. It does **not** claim a head-to-head win over mem0,
LangChain, MemGPT, or Zep — those are different-shaped systems (different embedders, different
infra, no shared hardware/dataset), and fabricating a comparison number without actually running
them would be exactly the "decorative, unverifiable intelligence" the project's own cut list exists
to prevent. What follows is what can be honestly measured today.

## 1. Recall quality: keyword-only vs semantic-only vs hybrid RRF

Run with `cargo run --release -p engram-bench --bin engram-bench` (`crates/engram-bench/src/main.rs`).

**Method:** 17 hand-labeled (query, target-fact) pairs plus 8 distractor facts (25 facts total,
Region::Semantic), seeded into a real `Memory` via the public `remember()` API — no synthetic
shortcuts. Three arms are run head-to-head against the *identical* corpus and query set:

- **keyword-only** — the real FTS5/BM25 query `recall_inner`'s keyword arm runs (same tokenization:
  every ≥2-char alphanumeric token, quoted and OR-joined), executed directly via a second read-only
  `rusqlite::Connection` to the same on-disk db file. No RRF, no semantic signal at all.
- **semantic-only** — exact cosine over the SAME stored embeddings (same connection), isolated. No
  keyword signal, no RRF.
- **hybrid** — Engram's actual, unmodified `Memory::recall()` (BM25 + semantic, Reciprocal Rank
  Fusion). What ships.

10 of the 17 queries are constructed to share **zero lexical overlap** with their target fact
(paraphrases and true synonyms, e.g. "purchasing a car recently" → "she bought a new automobile
last week") — a keyword index has exactly 0 chance on these *by construction*, isolating what
semantic signal specifically buys.

### Result (2026-07-05, offline `TrigramHashEmbedder`, the always-available bundled default)

| Arm | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |
|---|---|---|---|
| keyword-only (FTS5/BM25, isolated) | 65% (11/17) | 0.537 | 40% (4/10) |
| semantic-only (exact cosine, isolated) | 94% (16/17) | 0.779 | 90% (9/10) |
| hybrid (BM25 + semantic, RRF-fused — what Engram ships) | 88% (15/17) | 0.755 | 80% (8/10) |

**The uncontested win: on queries with zero shared words, keyword-only is stuck at 40% while
semantic and hybrid both roughly double it (80–90%).** That's the core, unambiguous claim hybrid
retrieval exists to make good on, and it holds.

**The honest surprise: semantic-only edges out hybrid by one case (16/17 vs 15/17) on this
particular query set with the trigram-hash embedder.** Traced with
`ENGRAM_BENCH_VERBOSE=1 ./target/release/engram-bench` (labeled per-arm, per-case hit/miss/rank
tracing — a permanent feature of the harness, not a one-off script): the single miss is "purchasing
a car recently" → "she bought a new automobile last week", a true-synonym case with zero shared
words *and* zero shared character trigrams. Semantic-only barely recalls it at rank 10 (the very
edge of the k=10 window). RRF fusion adds the keyword arm's candidate pool into the same fixed
top-10 window; because the semantic signal for this case was already marginal, the additional
keyword-surfaced candidates (irrelevant to this query, but real BM25 hits for *other* facts) win
enough of the fused ranking to push this one result just outside the top 10. This is a known
trade-off of rank-position fusion with a fixed output size, not a case where hybrid's semantic
signal was wrong — and it costs exactly one doubly-hard case (weak-semantic *and* zero-lexical) out
of seventeen. Expected to matter less with a stronger embedder (the static model2vec path, or a real
transformer via the gateway) where the correct match ranks higher than the k-cutoff to begin with,
and to matter more on corpora where BM25 has more opportunity to surface confident, wrong,
high-ranking candidates — worth re-checking as the query/corpus set grows.

**Practical implication:** hybrid is the right default because it's the only arm with no
catastrophic blind spot (keyword-only's 40% floor on paraphrases) while staying close to
semantic-only's ceiling — not because it wins every single case measured today.

Set `ENGRAM_STATIC_MODEL=<model2vec dir>` to also measure the static (model2vec, pure-Rust,
synonym-level) embedder path — no model directory ships in this repo or is fetched by this
benchmark, so that path is currently unmeasured on a fresh checkout; see
`crates/engram-memory/src/static_embed.rs` for the expected model format.

## 2. Scale: does scoping actually confine a query, at 40 projects × 10k rows?

Run with `cargo run --release --bin scale_bench -p engram-bench`
(`crates/engram-bench/src/bin/scale_bench.rs`; full write-up in `SCALE-BENCHMARK.md`).

This started as re-verification of a *disputed* claim from the 2026-07-05 design review ("40x scan
amplification" if scoping broke) — building a real benchmark to settle a two-sided disagreement
turned out to be the right call: the claim did not reproduce, and trusting either side without
measuring would have wasted real engineering time either building an unneeded index restructure or
shipping a real bug.

### Result (2026-07-05, current schema, release build)

```
Inserted 400,000 rows (40 projects × 10,000) in ~2.5s

recall_scoped(one project's ring):        10 hits in  ~4-63ms   (depends on query shape)
recall_scoped(whole-brain, no ring filter): 10 hits in ~110-260ms
```

SQLite's query planner applies `MULTI-INDEX OR` to `scope_clause()`'s union-of-rings predicate,
using `idx_facts_scope` per ring and merging results — a single project's query touches only that
project's ~10,001 rows (its own ring + the user-global ring) out of 400,001 total, not the whole
region across every project. **A single project stays fast regardless of how many OTHER big
projects exist on the same daemon** — the concrete "handles multiple big projects" claim, measured,
not asserted.

## 3. What isn't covered here yet

- **Real transformer/gateway-embedder recall quality** — needs a live provider; the offline harness
  above is what runs on every CI/dev machine with zero external dependencies.
- **Recall quality *at* scale** — today's two benchmarks measure quality (small corpus, rich
  labels) and scale (large corpus, structural timing) separately, not recall precision on a
  40-project/10k-row brain. A combined harness (the labeled query set embedded inside a scaled,
  multi-project brain) is a natural next addition, not yet built.
- **Skill-replay / procedural-memory quality** — the self-improving skill loop's A/B replay-score
  mechanism (`engram-skills`) is a different, already-real verification signal (a skill's own
  replay score, checkable via `engram skills show <id>`), not folded into this memory-recall suite.
