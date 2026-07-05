//! Scale benchmark - does `scope_clause()`'s union-of-rings predicate actually use
//! `idx_facts_scope` at realistic multi-project scale, against the CURRENT (post-migration)
//! schema?
//!
//! This exists to re-verify a disputed claim before committing engineering time to a query
//! rewrite: a candidate design justified restructuring `scope_clause()` with a "confirmed 40x
//! scan amplification at 40 projects x 10k rows" benchmark; an independent adversarial pass
//! reproduced the same shape against the current schema and did not see the amplification.
//! See docs/MEMORY-UPGRADE-PLAN.md, "A claim we do not carry forward unverified."
//!
//! Method: build the brain via the REAL `Memory::open` (so schema/indexes are byte-identical to
//! production), bulk-insert 40 projects x 10,000 synthetic rows directly (bypassing `remember()`'s
//! per-row ledger fsync, which would make the benchmark's own write path the bottleneck rather
//! than measuring recall), then run `EXPLAIN QUERY PLAN` plus real wall-clock timing on the exact
//! query shapes `recall_inner` and `current_with_prefix_scoped` build, both with and without
//! `ANALYZE` (Engram never runs `ANALYZE` anywhere today - checked by grep - so this also answers
//! whether that's a gap worth closing).

use std::sync::Arc;
use std::time::Instant;

use engram_core::{Ledger, ScopeCtx};
use engram_memory::embed::{quantize_binary, to_bytes};
use engram_memory::{Embedder, Memory, Region, TrigramHashEmbedder};
use rusqlite::{params, Connection};

const PROJECTS: usize = 40;
const ROWS_PER_PROJECT: usize = 10_000;
const REGIONS: [&str; 3] = ["semantic", "episodic", "identity"];

fn explain(conn: &Connection, sql: &str, binds: &[&dyn rusqlite::ToSql]) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let rows = stmt.query_map(binds, |r| r.get::<_, String>(3)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

fn uses_index(plan: &[String], index: &str) -> bool {
    plan.iter().any(|line| line.contains(index))
}

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("scale.db");
    let embedder = TrigramHashEmbedder::default();

    // 1. Real schema via the real crate - first-ever open on an empty file stamps the embed
    //    space and creates every table/index `init_schema` defines, with zero rows to migrate.
    {
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let _mem =
            Memory::open(&db_path, Arc::new(TrigramHashEmbedder::default()), ledger).unwrap();
    }

    // 2. Bulk-insert PROJECTS * ROWS_PER_PROJECT synthetic rows directly, bypassing remember()'s
    //    per-row ledger fsync (which would make the insert itself the bottleneck, not what this
    //    benchmark measures). One shared embedding vector for every row: content-plan/timing only,
    //    not recall-quality, is under test here.
    let shared_vec = embedder.embed("synthetic benchmark row");
    let shared_emb = to_bytes(&shared_vec);
    let shared_bin = quantize_binary(&shared_vec);
    {
        let mut conn = Connection::open(&db_path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        let insert_start = Instant::now();
        let tx = conn.transaction().unwrap();
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO facts \
                     (region, text, importance, taint, tier, content_hash, embedding, embedding_bin, \
                      created_ms, last_access_ms, access_count, deleted, scope_kind, scope_id) \
                     VALUES (?1, ?2, 0.5, 'trusted', 'warm', ?3, ?4, ?5, ?6, ?6, 0, 0, 'project', ?7)",
                )
                .unwrap();
            let mut n: i64 = 0;
            for p in 0..PROJECTS {
                let scope_id = format!("project-{p}");
                for i in 0..ROWS_PER_PROJECT {
                    let region = REGIONS[i % REGIONS.len()];
                    let text = format!("synthetic fact {p}-{i} about the project's ongoing work");
                    let hash = format!("hash-{p}-{i}");
                    stmt.execute(params![
                        region,
                        text,
                        hash,
                        shared_emb,
                        shared_bin,
                        1_700_000_000_000i64 + n,
                        scope_id,
                    ])
                    .unwrap();
                    n += 1;
                }
            }
        }
        tx.commit().unwrap();
        println!(
            "Inserted {} rows in {:.2}s\n",
            PROJECTS * ROWS_PER_PROJECT,
            insert_start.elapsed().as_secs_f64()
        );
    }

    // 3. A few user-global rows too (id/identity facts), and one target project's Identity rows
    //    for the current_with_prefix_scoped test.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO facts (region, text, importance, taint, tier, content_hash, embedding, \
             embedding_bin, created_ms, last_access_ms, access_count, deleted, scope_kind, scope_id) \
             VALUES ('identity', 'the user lives in Berlin', 0.5, 'trusted', 'warm', 'uh1', ?1, ?2, \
             1700000000000, 1700000000000, 0, 0, 'user', '')",
            params![shared_emb, shared_bin],
        )
        .unwrap();
    }

    let target_scope = ScopeCtx::project("project-17");
    // Mirrors store.rs's `scope_clause()` EXACTLY, including the outer wrapping parens it adds
    // around the whole OR-of-rings clause (`format!("({})", parts.join(" OR "))`) - omitting
    // that outer paren changes AND/OR precedence and would silently test a different, wrong
    // query than the one the real code executes.
    let scope_sql =
        "((scope_kind = 'user' AND scope_id = ?) OR (scope_kind = 'project' AND scope_id = ?))";
    let coarse_sql = format!(
        "SELECT id, embedding_bin FROM facts WHERE deleted = 0 AND superseded_by IS NULL \
         AND region IN ('semantic','episodic','identity','procedural') AND {scope_sql}"
    );
    let prefix_sql = format!(
        "SELECT id FROM facts WHERE region = ? AND deleted = 0 AND superseded_by IS NULL \
         AND text LIKE ? AND {scope_sql}"
    );

    println!(
        "## Query-plan check: scope_clause() union-of-rings, {} project rings active\n",
        target_scope.rings().len()
    );
    for analyzed in [false, true] {
        let conn = Connection::open(&db_path).unwrap();
        if analyzed {
            conn.execute_batch("ANALYZE;").unwrap();
        }
        let plan = explain(
            &conn,
            &coarse_sql,
            &[&"" as &dyn rusqlite::ToSql, &"project-17"],
        );
        println!(
            "- ANALYZE={analyzed}: {}",
            if uses_index(&plan, "idx_facts_scope") {
                "USES idx_facts_scope"
            } else if uses_index(&plan, "idx_facts_region") {
                "falls back to idx_facts_region (scope filtered row-by-row)"
            } else {
                "SCANs facts (no index used)"
            }
        );
        for line in &plan {
            println!("    {line}");
        }

        // Real wall-clock timing of the exact SELECT, both for one project's ring and (as a
        // reference point) for a whole-brain scan with no scope filter at all.
        let start = Instant::now();
        let mut stmt = conn.prepare(&coarse_sql).unwrap();
        let n_project = stmt
            .query_map(params!["", "project-17"], |r| r.get::<_, i64>(0))
            .unwrap()
            .count();
        let project_elapsed = start.elapsed();

        let whole_sql = coarse_sql.replace(
            "(scope_kind = 'user' AND scope_id = ?) OR (scope_kind = 'project' AND scope_id = ?)",
            "1 = 1",
        );
        let start = Instant::now();
        let mut stmt = conn.prepare(&whole_sql).unwrap();
        let n_whole = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap().count();
        let whole_elapsed = start.elapsed();

        println!(
            "  timing: project-scoped {n_project} rows in {:.2}ms, whole-brain {n_whole} rows in {:.2}ms (amplification if scope filter weren't used: {:.1}x by row count)\n",
            project_elapsed.as_secs_f64() * 1000.0,
            whole_elapsed.as_secs_f64() * 1000.0,
            n_whole as f64 / n_project.max(1) as f64,
        );
    }

    println!("## Query-plan check: current_with_prefix_scoped()'s LIKE-based query\n");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("ANALYZE;").unwrap();
        let plan = explain(
            &conn,
            &prefix_sql,
            &[
                &"identity" as &dyn rusqlite::ToSql,
                &"i live %",
                &"",
                &"project-17",
            ],
        );
        println!(
            "- {}",
            if uses_index(&plan, "idx_facts_scope") {
                "USES idx_facts_scope"
            } else if uses_index(&plan, "idx_facts_region") {
                "uses idx_facts_region only (scope + LIKE filtered row-by-row - matches the audit finding)"
            } else {
                "SCANs facts"
            }
        );
        for line in &plan {
            println!("    {line}");
        }
    }

    println!("\n## End-to-end recall_scoped() timing through the real Memory API\n");
    {
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let mem = Memory::open(&db_path, Arc::new(TrigramHashEmbedder::default()), ledger).unwrap();
        let regions = [Region::Semantic, Region::Episodic, Region::Identity];
        let start = Instant::now();
        let hits = mem
            .recall_scoped("ongoing project work", &regions, 10, &target_scope)
            .unwrap();
        println!(
            "  recall_scoped(project-17 ring): {} hits in {:.1}ms",
            hits.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
        let whole = ScopeCtx::any();
        let start = Instant::now();
        let hits = mem
            .recall_scoped("ongoing project work", &regions, 10, &whole)
            .unwrap();
        println!(
            "  recall_scoped(whole-brain, no ring filter): {} hits in {:.1}ms",
            hits.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}
