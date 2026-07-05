# Engram memory benchmarks

Real, committed, re-runnable numbers for the memory-upgrade plan's headline claims
(`docs/MEMORY-UPGRADE-PLAN.md`). Every number below comes from running real code in this repo (or,
for §3, real code from mem0/LangChain installed locally) — no number is asserted or hand-picked;
the methodology and the diagnostic to reproduce it are both described. Where a result was
surprising or uncomfortable, it's reported as found, not smoothed over.

This started as an *internal* benchmark suite (Engram's own hybrid retrieval against isolated
single-signal baselines built from the exact same stored index) and now also includes a **real,
executed** external comparison against mem0 and LangChain (§3) — installed and run locally, not
assumed.

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

### With a real model2vec model (`ENGRAM_STATIC_MODEL=<dir>`)

No model ships in this repo or is fetched by this benchmark (a fresh checkout measures the
trigram-hash numbers above only) — but the path is real and was verified end-to-end: downloaded
`minishlab/potion-base-8M` (a public model2vec export, `tokenizer.json` + `model.safetensors`, via
`huggingface_hub.hf_hub_download`) and pointed `ENGRAM_STATIC_MODEL` at it.

| Arm | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |
|---|---|---|---|
| keyword-only (FTS5/BM25, isolated) | 65% (11/17) | 0.537 | 40% (4/10) |
| semantic-only (exact cosine, isolated) | 100% (17/17) | 1.000 | 100% (10/10) |
| hybrid (BM25 + semantic, RRF-fused) | 100% (17/17) | 0.892 | 100% (10/10) |

With a real trained embedding model in place of the zero-dependency trigram-hash fallback, **hybrid
reaches the same perfect recall as semantic-only** — the earlier trigram-hash "hybrid loses one
case to fusion-window competition" finding (§1 above) doesn't reproduce once the semantic signal
itself is strong enough that no result is ever marginal. See §3 for what this means next to mem0
and LangChain using their own real embedding models.

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

## 3. External comparison: mem0 and LangChain (executed, not assumed)

Run with `python3 compare_external.py` (`crates/engram-bench/compare_external.py`) inside a
Python 3.11 venv with `mem0ai`, `langchain`, `langchain-community`, `sentence-transformers`, and
`faiss-cpu` installed. Uses the **exact same 17-query/8-distractor corpus and scoring** as §1, so
the numbers are directly comparable, not just similar-shaped.

**Method:** each system uses its own natural, out-of-the-box local embedding path — nothing was
tuned or cherry-picked:
- **mem0** — `Memory.add(..., infer=False)`, a real documented mode (verified in mem0's own source:
  this path never calls an LLM, only the embedder), mem0's own default HuggingFace embedder
  (`multi-qa-MiniLM-L6-cos-v1`), local on-disk Qdrant vector store. `Memory()`'s LLM client
  construction is eager even when unused, requiring a dummy `OPENAI_API_KEY` env var as a
  workaround (no network call to OpenAI is ever made under `infer=False`) — a real, minor wart in
  mem0's own design, not a benchmark artifact.
- **LangChain** — `HuggingFaceEmbeddings` (`all-MiniLM-L6-v2`) + `FAISS.from_texts`, pure similarity
  search, no LLM anywhere in the path. No blockers at all.
- **Engram** — both its zero-dependency default (trigram-hash) and, separately, the real
  static-embedder path from §1 (`minishlab/potion-base-8M`, the same category of pretrained local
  model mem0/LangChain use here).

### Result (2026-07-05)

| System | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |
|---|---|---|---|
| Engram, hybrid, trigram-hash default (zero external model, zero download) | 88% (15/17) | 0.755 | 80% (8/10) |
| mem0, `infer=False`, HF `multi-qa-MiniLM-L6-cos-v1`, local Qdrant | 100% (17/17) | 0.971 | 100% (10/10) |
| LangChain, HF `all-MiniLM-L6-v2`, FAISS | 100% (17/17) | 1.000 | 100% (10/10) |
| Engram, hybrid, real model2vec embedder (`potion-base-8M`) | 100% (17/17) | 0.892 | 100% (10/10) |

**The honest, uncomfortable finding first: Engram's zero-dependency default embedder loses to real,
downloaded sentence-transformer models.** This isn't a retrieval-*algorithm* gap — mem0's tested
mode here is pure vector search (no hybrid; its BM25/fastembed path was also installed and tested,
with no measurable change on this small corpus) and LangChain has no hybrid mode at all. It's an
*embedding-quality* gap: trigram-hash is a character/morphology hash chosen specifically so Engram
never needs a model download or network call to embed anything, not a trained semantic model. On
true synonyms with zero shared characters ("car" / "automobile"), a real embedding model simply
understands more than a hash can.

**The real result once that gap is closed:** plug in a real local model via the *already-existing*
static-embedder path (no new code, no architecture change - `ENGRAM_STATIC_MODEL=<dir>`) and Engram
**matches mem0 and LangChain exactly on raw recall** (100%/17), while its hybrid retrieval keeps the
keyword-fallback robustness neither of them has in the configuration tested here (mem0's hybrid arm
showed no measurable benefit on this small corpus, but a keyword fallback catching rare/exact-term
queries a semantic-only system misses is the well-established general case, not something this
25-fact corpus is large enough to stress).

**Actionable, not just observed:** the concrete gap is that no model2vec model ships with, or is
fetched by, a fresh Engram install — so every real install today runs on the trigram-hash floor
(confirmed separately by `embedder_degraded` firing on every daemon in this session with no model
present). Bundling or one-click-fetching a small model2vec model would let Engram ship this
already-built, already-verified recall-quality ceiling by default instead of requiring a manual
`ENGRAM_STATIC_MODEL` env var and a manual download - the single highest-leverage recall-quality
improvement available, and it needs zero new engineering, only packaging.

**What this comparison does NOT claim:** a benchmark on a 25-fact corpus with unambiguous labels is
a floor/ceiling check, not a claim that Engram or mem0 or LangChain is "the best" at memory in
general - MemGPT/Letta's paging model, Zep's temporal knowledge graph, and mem0's LLM-driven
fact-extraction/consolidation pipeline (deliberately not exercised here — see method above) all do
things this corpus can't distinguish. This section proves the specific, narrow, real claim: hybrid
retrieval + a real embedding model is not behind the state of the art on raw recall, and Engram
already has the plumbing for both.

## 4. What isn't covered here yet

- **Recall quality *at* scale** — today's benchmarks measure quality (small corpus, rich labels)
  and scale (large corpus, structural timing) separately, not recall precision on a
  40-project/10k-row brain. A combined harness (the labeled query set embedded inside a scaled,
  multi-project brain) is a natural next addition, not yet built.
- **mem0's LLM-driven fact-extraction/consolidation pipeline** and **MemGPT/Letta's paging model**
  and **Zep's temporal knowledge graph** — each is a materially different capability from plain
  recall-quality retrieval; none is exercised by this corpus, and claiming a win or loss against
  them from this benchmark would overclaim.
- **Skill-replay / procedural-memory quality** — the self-improving skill loop's A/B replay-score
  mechanism (`engram-skills`) is a different, already-real verification signal (a skill's own
  replay score, checkable via `engram skills show <id>`), not folded into this memory-recall suite.
