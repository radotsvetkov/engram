# Scale benchmark: scope-index verification

Produced by `cargo run --release --bin scale_bench -p engram-bench`
(`crates/engram-bench/src/bin/scale_bench.rs`).

## Why this exists

A candidate architecture design (generated during the 2026-07-05 memory-system audit) justified
restructuring `scope_clause()`'s union-of-rings predicate with a claim of "confirmed 40x scan
amplification at 40 projects x 10k rows" — i.e. that a single project's recall query could not use
`idx_facts_scope` once 2+ scope rings were active, and fell back to scanning the whole region across
every project on the daemon. A second, independently-run adversarial judge pass on that same design
reproduced the experiment against the CURRENT (already-indexed) schema and did not see the
amplification. This benchmark is the tie-breaker, run against the real schema via the real `Memory`
API, with the exact SQL shapes copied verbatim (including operator-precedence-critical parens) from
`crates/engram-memory/src/store.rs`.

## Method

1. `Memory::open` on an empty file — real schema, real indexes, byte-identical to production.
2. Bulk-insert 40 projects x 10,000 rows directly (bypassing `remember()`'s per-row ledger fsync,
   which would make the *insert* the bottleneck rather than what's under test).
3. `EXPLAIN QUERY PLAN` plus real wall-clock timing on the exact query text `recall_inner`'s coarse
   semantic arm and `current_with_prefix_scoped` build, with and without `ANALYZE` (Engram never
   runs `ANALYZE` anywhere today — confirmed by grep).
4. End-to-end timing through the real, public `Memory::recall_scoped` API.

## Result (2026-07-05, this workspace, release build)

```
Inserted 400000 rows in 2.2s

## scope_clause() union-of-rings, 2 project rings active
- ANALYZE=false: USES idx_facts_scope
    MULTI-INDEX OR
    SEARCH facts USING INDEX idx_facts_scope (scope_kind=? AND scope_id=? AND deleted=?)
    SEARCH facts USING INDEX idx_facts_scope (scope_kind=? AND scope_id=? AND deleted=?)
  timing: project-scoped 10001 rows in 3.7ms, whole-brain 400001 rows in 257.8ms

- ANALYZE=true: USES idx_facts_scope (same plan)
  timing: project-scoped 10001 rows in 3.8ms, whole-brain 400001 rows in 106.2ms

## current_with_prefix_scoped()'s LIKE-based query
- USES idx_facts_scope (same MULTI-INDEX OR plan)

## recall_scoped() end-to-end, through the real Memory API
  project-17 ring: 10 hits in 61.0ms
  whole-brain (ScopeCtx::any(), no ring filter): 10 hits in 183.2ms
```

## Verdict

**The disputed claim does not reproduce.** SQLite's query planner already applies its OR
optimization (`MULTI-INDEX OR`) to `scope_clause()`'s union-of-rings predicate, using
`idx_facts_scope` per ring and merging the results — a single project's recall query touches only
that project's own rows (~10,001 here: the project ring plus the user-global ring) out of 400,001
total, not the whole region across every project. `ANALYZE` makes no difference to the chosen plan
(only to the whole-brain reference timing, likely via better page-cache/row-estimate behavior — not
worth adding purely for this query shape).

`current_with_prefix_scoped()`'s `text LIKE ?` clause was independently flagged as unable to use the
composite index regardless of the outer benchmark's outcome — **this also does not reproduce**.
SQLite satisfies the scope OR-clause via the index first, then applies `region = ?` and `text LIKE ?`
as residual filters over the small scope-matched candidate set, which is fast at this scale because
the candidate set is already small.

**No scope-index restructuring work is needed.** `docs/MEMORY-UPGRADE-PLAN.md`'s Task 0 is resolved:
the plan's disputed claim is dropped, not implemented. The ~3x (not 40x) gap between
project-scoped (61ms) and whole-brain (183ms) `recall_scoped()` timing is expected and appropriate —
`ScopeCtx::any()` is the deliberate "show everything" whole-brain view (used only by the Atlas
inspection UI, never for model-facing recall), not a bug.

Re-run this benchmark (`cargo run --release --bin scale_bench -p engram-bench`) if `scope_clause()`,
the `facts` schema, or its indexes ever change, to catch a real regression before it ships.
