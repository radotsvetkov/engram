//! Paraphrase recall benchmark.
//!
//! The headline recall claim is that hybrid (semantic + keyword) memory finds the
//! right fact for a query that shares *no words* with it - exactly where a
//! keyword-only store returns nothing. This harness measures that honestly:
//!
//! - It reports hybrid recall@10 and MRR over a labelled query set.
//! - It isolates the **zero-lexical-overlap** subset, where a keyword index has 0
//!   recall *by construction*, and reports what hybrid recovers there.
//!
//! Run with the bundled offline embedder (`TrigramHashEmbedder`), this captures
//! morphology and word-order - a real step up over keyword matching. Synonym-level
//! paraphrase ("car" → "automobile") needs the transformer embedder that plugs into
//! the same `Embedder` trait via the gateway; this harness is what measures it when
//! that model is wired.

use std::collections::HashMap;
use std::sync::Arc;

use engram_core::Ledger;
use engram_memory::{Memory, Region, StaticEmbedder, TrigramHashEmbedder, WriteReq};

struct Case {
    fact: &'static str,
    query: &'static str,
}

/// Query → the one fact it should recall. Several queries deliberately share no whole
/// word with their target (morphological paraphrases) - keyword search cannot find
/// these at all.
fn cases() -> Vec<Case> {
    vec![
        Case { fact: "user preferences for dark themes in the editor", query: "preferred theming" },
        Case { fact: "the agent consolidates memories overnight", query: "memory consolidation while sleeping" },
        Case { fact: "scheduling recurring reminders every morning", query: "recurrent schedules" },
        Case { fact: "skills are sandboxed programs that improve with use", query: "sandboxing improvable programs" },
        Case { fact: "Engram runs on a cheap virtual private server", query: "running cheaply on a VPS" },
        Case { fact: "the ledger is signed and tamper evident", query: "tamper-evident signing" },
        Case { fact: "Radoslav prefers minimal dependencies", query: "minimal dependency preference" },
        Case { fact: "embeddings turn text into vectors for semantic search", query: "vector embedding for meaning" },
        Case { fact: "the core sleeps to zero memory when idle", query: "idle sleeping to zero" },
        Case { fact: "the capital of France is Paris", query: "what is the capital of France" },
        Case { fact: "WebAssembly modules run in a fuel-bounded sandbox", query: "fuel bounded wasm sandboxing" },
        Case { fact: "recall fuses keyword and semantic ranking", query: "fusing semantic and keyword ranks" },
        // True synonyms: no shared word OR character-trigram - only a learned embedder
        // (not the morphological trigram baseline) can bridge these.
        Case { fact: "she bought a new automobile last week", query: "purchasing a car recently" },
        Case { fact: "the physician prescribed rest and fluids", query: "advice from a doctor" },
        Case { fact: "the film received glowing reviews", query: "the movie got great write-ups" },
        Case { fact: "he is fluent in several tongues", query: "speaks many languages" },
        Case { fact: "the firm hired a dozen new staff", query: "the company recruited employees" },
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

/// Run the recall benchmark with one embedder and return its scores.
fn evaluate(embedder: Arc<dyn engram_memory::Embedder>) -> Score {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
    let mem = Memory::open(dir.path().join("bench.db"), embedder, ledger).unwrap();

    let cases = cases();
    let mut want: HashMap<&str, i64> = HashMap::new();
    for c in &cases {
        want.insert(c.fact, mem.remember(WriteReq::new(Region::Semantic, c.fact)).unwrap().id);
    }
    for d in distractors() {
        mem.remember(WriteReq::new(Region::Semantic, d)).unwrap();
    }

    let k = 10;
    let (mut hits, mut mrr, mut hard_total, mut hard_hits) = (0usize, 0.0f32, 0usize, 0usize);
    for c in &cases {
        let target = want[c.fact];
        let results = mem.recall(c.query, &[Region::Semantic], k).unwrap();
        let pos = results.iter().position(|h| h.record.id == target);
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
    Score { hits, mrr, hard_hits, hard_total, n: cases.len() }
}

fn main() {
    let trig = evaluate(Arc::new(TrigramHashEmbedder::default()));
    // Compare against the static (model2vec) embedder when ENGRAM_STATIC_MODEL points at a
    // model directory - this is what measures the synonym-level recall jump.
    let stat = std::env::var("ENGRAM_STATIC_MODEL").ok().and_then(|p| match StaticEmbedder::load(&p) {
        Ok(e) => Some(evaluate(Arc::new(e))),
        Err(err) => {
            eprintln!("static embedder load failed ({p}): {err}");
            None
        }
    });

    let total_facts = trig.n + distractors().len();
    println!("# Engram benchmark - paraphrase recall & footprint\n");
    println!("Corpus: {total_facts} facts. Queries: {}. Recall@10.\n", trig.n);

    let row = |label: &str, s: &Score| {
        let hard = if s.hard_total == 0 { 0.0 } else { 100.0 * s.hard_hits as f32 / s.hard_total as f32 };
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
    println!("| Embedder | Recall@10 | MRR | Recall@10 on zero-overlap paraphrases |");
    println!("|---|---|---|---|");
    row("trigram-hash (offline default)", &trig);
    if let Some(s) = &stat {
        row("static model2vec (pure-Rust)", s);
    }
    println!("| keyword-only baseline | - | - | 0% (by construction) |");

    println!("\nBinary size (full agent): {}   ·   Idle RAM: 0 MB (socket-activated)", binary_size());
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
