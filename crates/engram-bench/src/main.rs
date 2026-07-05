//! Paraphrase recall benchmark.
//!
//! The headline recall claim is that hybrid (semantic + keyword, RRF-fused) memory beats EITHER
//! signal alone - and specifically finds the right fact for a query that shares *no words* with
//! it, exactly where a keyword-only store returns nothing. This harness measures all three arms
//! honestly, head to head, on the identical corpus and query set:
//!
//! - **keyword-only**: the real FTS5/BM25 query `recall_inner`'s keyword arm runs, isolated (no
//!   RRF, no semantic signal at all) - what a grep/full-text-only memory store would return.
//! - **semantic-only**: exact cosine over the SAME stored embeddings, isolated (no keyword signal,
//!   no RRF) - what a pure-vector-search memory store would return.
//! - **hybrid**: Engram's actual `Memory::recall` (BM25 + semantic, fused by RRF).
//!
//! It also isolates the **zero-lexical-overlap** subset, where keyword-only has 0 recall *by
//! construction*, and reports what semantic-only and hybrid recover there.
//!
//! Run with the bundled offline embedder (`TrigramHashEmbedder`), this captures morphology and
//! word-order - a real step up over keyword matching. Synonym-level paraphrase ("car" →
//! "automobile") needs the transformer embedder that plugs into the same `Embedder` trait via the
//! gateway; this harness is what measures it when that model is wired (or the static model2vec
//! embedder, via `ENGRAM_STATIC_MODEL`).

use std::collections::HashMap;
use std::sync::Arc;

use engram_core::Ledger;
use engram_memory::{
    cosine, embed::from_bytes, Embedder, Memory, Region, StaticEmbedder, TrigramHashEmbedder,
    WriteReq,
};

struct Case {
    fact: &'static str,
    query: &'static str,
}

/// Query → the one fact it should recall. Several queries deliberately share no whole
/// word with their target (morphological paraphrases) - keyword search cannot find
/// these at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            fact: "user preferences for dark themes in the editor",
            query: "preferred theming",
        },
        Case {
            fact: "the agent consolidates memories overnight",
            query: "memory consolidation while sleeping",
        },
        Case {
            fact: "scheduling recurring reminders every morning",
            query: "recurrent schedules",
        },
        Case {
            fact: "skills are sandboxed programs that improve with use",
            query: "sandboxing improvable programs",
        },
        Case {
            fact: "Engram runs on a cheap virtual private server",
            query: "running cheaply on a VPS",
        },
        Case {
            fact: "the ledger is signed and tamper evident",
            query: "tamper-evident signing",
        },
        Case {
            fact: "Radoslav prefers minimal dependencies",
            query: "minimal dependency preference",
        },
        Case {
            fact: "embeddings turn text into vectors for semantic search",
            query: "vector embedding for meaning",
        },
        Case {
            fact: "the core sleeps to zero memory when idle",
            query: "idle sleeping to zero",
        },
        Case {
            fact: "the capital of France is Paris",
            query: "what is the capital of France",
        },
        Case {
            fact: "WebAssembly modules run in a fuel-bounded sandbox",
            query: "fuel bounded wasm sandboxing",
        },
        Case {
            fact: "recall fuses keyword and semantic ranking",
            query: "fusing semantic and keyword ranks",
        },
        // True synonyms: no shared word OR character-trigram - only a learned embedder
        // (not the morphological trigram baseline) can bridge these.
        Case {
            fact: "she bought a new automobile last week",
            query: "purchasing a car recently",
        },
        Case {
            fact: "the physician prescribed rest and fluids",
            query: "advice from a doctor",
        },
        Case {
            fact: "the film received glowing reviews",
            query: "the movie got great write-ups",
        },
        Case {
            fact: "he is fluent in several tongues",
            query: "speaks many languages",
        },
        Case {
            fact: "the firm hired a dozen new staff",
            query: "the company recruited employees",
        },
    ]
}

/// Distractors raise the bar - recall@10 must pick the target out of a fuller brain.
fn distractors() -> Vec<&'static str> {
    vec![
        "the weather in Berlin is mild in spring",
        "coffee is brewed from roasted beans",
        "the train departs from platform nine",
        "photosynthesis converts light into energy",
        "the meeting was rescheduled to Thursday",
        "mountains are formed by tectonic activity",
        "the recipe calls for two cups of flour",
        "satellites orbit the planet every ninety minutes",
    ]
}

const STOP: &[&str] = &[
    "the", "a", "an", "is", "are", "for", "in", "on", "of", "to", "and", "with", "that", "into",
    "while", "every", "what", "at", "it", "as", "by",
];

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2 && !STOP.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// Does the query share at least one content word with the fact? If not, a keyword
/// index has zero chance of recalling it.
fn lexical_overlap(query: &str, fact: &str) -> bool {
    let f = tokens(fact);
    tokens(query).iter().any(|t| f.contains(t))
}

struct Score {
    hits: usize,
    mrr: f32,
    hard_hits: usize,
    hard_total: usize,
    n: usize,
}

/// Score a set of ranked-id lists (one per case) against their targets - shared scoring logic for
/// all three arms (keyword-only, semantic-only, hybrid), so the numbers are directly comparable.
/// `label` is only used for `ENGRAM_BENCH_VERBOSE=1` per-case tracing (which arm missed which
/// query, and at what rank a hit landed) - set it to see exactly where an arm's numbers come from
/// instead of trusting the aggregate percentage.
fn score(
    label: &str,
    cases: &[Case],
    want: &HashMap<&str, i64>,
    ranked: impl Fn(&Case) -> Vec<i64>,
) -> Score {
    let verbose = std::env::var("ENGRAM_BENCH_VERBOSE").is_ok();
    let (mut hits, mut mrr, mut hard_total, mut hard_hits) = (0usize, 0.0f32, 0usize, 0usize);
    for c in cases {
        let target = want[c.fact];
        let results = ranked(c);
        let pos = results.iter().position(|&id| id == target);
        if verbose {
            eprintln!(
                "  [{label}] {} pos={pos:?} query={:?}",
                if pos.is_some() { "HIT " } else { "MISS" },
                c.query
            );
        }
        let hard = !lexical_overlap(c.query, c.fact);
        if hard {
            hard_total += 1;
        }
        if let Some(p) = pos {
            hits += 1;
            mrr += 1.0 / (p as f32 + 1.0);
            if hard {
                hard_hits += 1;
            }
        }
    }
    Score {
        hits,
        mrr,
        hard_hits,
        hard_total,
        n: cases.len(),
    }
}

/// The real FTS5/BM25 query `recall_inner`'s keyword arm runs (same tokenization as the private
/// `build_match`: each >=2-char alphanumeric token, quoted and OR-joined), isolated - no RRF, no
/// semantic signal. What a keyword/grep-only memory store would return.
fn recall_keyword_only(conn: &rusqlite::Connection, query: &str, k: usize) -> Vec<i64> {
    let toks: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| format!("\"{t}\""))
        .collect();
    if toks.is_empty() {
        return Vec::new();
    }
    let match_q = toks.join(" OR ");
    let mut stmt = conn
        .prepare(
            "SELECT facts_fts.rowid FROM facts_fts JOIN facts f ON f.id = facts_fts.rowid \
             WHERE facts_fts MATCH ?1 AND f.deleted = 0 ORDER BY bm25(facts_fts) LIMIT ?2",
        )
        .unwrap();
    stmt.query_map(rusqlite::params![match_q, k as i64], |r| r.get::<_, i64>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

/// Exact cosine over the SAME stored embeddings, isolated - no keyword signal, no RRF. What a pure
/// vector-search memory store would return.
fn recall_semantic_only(
    conn: &rusqlite::Connection,
    embedder: &dyn Embedder,
    query: &str,
    k: usize,
) -> Vec<i64> {
    let q = embedder.embed(query);
    let mut stmt = conn
        .prepare("SELECT id, embedding FROM facts WHERE deleted = 0")
        .unwrap();
    let mut scored: Vec<(f32, i64)> = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|(id, blob)| (cosine(&q, &from_bytes(&blob)), id))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored.into_iter().map(|(_, id)| id).collect()
}

struct Scores {
    keyword: Score,
    semantic: Score,
    hybrid: Score,
}

/// Run all three arms (keyword-only, semantic-only, hybrid) with one embedder on the identical
/// corpus and query set, so the comparison is head-to-head, not three separately-run numbers.
fn evaluate(embedder: Arc<dyn Embedder>) -> Scores {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench.db");
    let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
    let mem = Memory::open(db_path.clone(), embedder.clone(), ledger).unwrap();

    let cases = cases();
    let mut want: HashMap<&str, i64> = HashMap::new();
    for c in &cases {
        want.insert(
            c.fact,
            mem.remember(WriteReq::new(Region::Semantic, c.fact))
                .unwrap()
                .id,
        );
    }
    for d in distractors() {
        mem.remember(WriteReq::new(Region::Semantic, d)).unwrap();
    }

    let k = 10;
    // A second, read-only connection to the SAME db file: WAL mode lets a reader see the writer's
    // committed rows without touching Memory's private connection - keyword-only/semantic-only run
    // the real stored FTS index and embeddings, not a separately-built index.
    let raw = rusqlite::Connection::open(&db_path).unwrap();

    let keyword = score("keyword", &cases, &want, |c| {
        recall_keyword_only(&raw, c.query, k)
    });
    let semantic = score("semantic", &cases, &want, |c| {
        recall_semantic_only(&raw, embedder.as_ref(), c.query, k)
    });
    let hybrid = score("hybrid", &cases, &want, |c| {
        mem.recall(c.query, &[Region::Semantic], k)
            .unwrap()
            .iter()
            .map(|h| h.record.id)
            .collect()
    });
    Scores {
        keyword,
        semantic,
        hybrid,
    }
}

fn main() {
    let trig = evaluate(Arc::new(TrigramHashEmbedder::default()));
    // Compare against the static (model2vec) embedder when ENGRAM_STATIC_MODEL points at a
    // model directory - this is what measures the synonym-level recall jump.
    let stat =
        std::env::var("ENGRAM_STATIC_MODEL")
            .ok()
            .and_then(|p| match StaticEmbedder::load(&p) {
                Ok(e) => Some(evaluate(Arc::new(e))),
                Err(err) => {
                    eprintln!("static embedder load failed ({p}): {err}");
                    None
                }
            });

    let total_facts = trig.hybrid.n + distractors().len();
    println!("# Engram benchmark - paraphrase recall & footprint\n");
    println!(
        "Corpus: {total_facts} facts. Queries: {}. Recall@10.\n",
        trig.hybrid.n
    );

    let row = |label: &str, s: &Score| {
        let hard = if s.hard_total == 0 {
            0.0
        } else {
            100.0 * s.hard_hits as f32 / s.hard_total as f32
        };
        println!(
            "| {label} | {:.0}% ({}/{}) | {:.3} | {:.0}% ({}/{}) |",
            100.0 * s.hits as f32 / s.n as f32,
            s.hits,
            s.n,
            s.mrr / s.n as f32,
            hard,
            s.hard_hits,
            s.hard_total
        );
    };
    println!("## Head-to-head: keyword-only vs semantic-only vs hybrid (offline trigram-hash embedder)\n");
    println!("| Arm | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |");
    println!("|---|---|---|---|");
    row("keyword-only (FTS5/BM25, isolated)", &trig.keyword);
    row("semantic-only (exact cosine, isolated)", &trig.semantic);
    row(
        "hybrid (BM25 + semantic, RRF-fused - what Engram ships)",
        &trig.hybrid,
    );
    if let Some(s) = &stat {
        println!("\n## With the static model2vec embedder (synonym-level, pure-Rust)\n");
        println!("| Arm | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |");
        println!("|---|---|---|---|");
        row("keyword-only (FTS5/BM25, isolated)", &s.keyword);
        row("semantic-only (exact cosine, isolated)", &s.semantic);
        row("hybrid (BM25 + semantic, RRF-fused)", &s.hybrid);
    }

    println!(
        "\nBinary size (full agent): {}   ·   Idle RAM: 0 MB (socket-activated)",
        binary_size()
    );
    if stat.is_none() {
        println!(
            "\nNote: set ENGRAM_STATIC_MODEL=<model2vec dir> to measure the static embedder's \
             synonym-level recall (this run shows the offline trigram baseline only)."
        );
    }
}

fn binary_size() -> String {
    for p in ["target/release/engramd", "../target/release/engramd"] {
        if let Ok(m) = std::fs::metadata(p) {
            return format!("{:.1} MB", m.len() as f64 / 1_048_576.0);
        }
    }
    "build --release to measure".to_string()
}
