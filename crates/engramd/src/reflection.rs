//! Grounded reflection (Phase D of docs/MEMORY-UPGRADE-PLAN.md) - the one genuinely new reasoning
//! capability in the memory-upgrade plan. Extends the hourly consolidation tick: when it finds a
//! small, bounded, co-scoped group of related Trusted-only facts (via
//! [`engram_memory::Memory::reflection_candidates`] - a greedy pairwise-cosine grouping over the
//! SAME warm/stale/low-importance candidate set `consolidate()` already scans, deliberately NOT
//! RAPTOR-style clustering, per the locked decision in docs/MEMORY-UPGRADE-PLAN.md §5), it makes
//! exactly one bounded LLM call per group using `dissent.rs`'s exact citation-and-strip-
//! hallucination discipline (`crate::citation`): list the candidates numbered, require the model to
//! cite which ones it drew on and state what they combine into, and drop the output entirely if the
//! reply doesn't parse or the model didn't actually offer a synthesis.
//!
//! Ships opt-in, default OFF (`security.auto_reflect`, matching the `auto_distill_skills` pattern),
//! and NEVER fires on Untrusted-tainted memories - `reflection_candidates` only ever returns
//! Trusted-provenance rows, and the synthesized fact it writes is always taint Trusted too.
//!
//! Like `contradiction.rs`, the citation here proves "the model looked at these specific rows" -
//! the resulting fact is a synthesis worth surfacing, not an infallible truth. The confirmation-UI
//! rule from §5 applies just as much here: a reflection must never be visually indistinguishable
//! from a directly-witnessed fact (see `metadata.reflection` on the written row, and
//! [`engram_memory::Record::tree_level`] = 1).

use std::time::Duration;

use engram_core::Taint;
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use engram_memory::{Memory, Record, Region, Scope, WriteReq};
use serde_json::json;

/// How close related candidates must be (cosine similarity) to be greedily grouped together -
/// deliberately looser than contradiction-detection's threshold, since reflection groups facts
/// that are related, not necessarily near-paraphrases of each other.
const CLUSTER_SIMILARITY: f32 = 0.5;
/// Hard bound on how many single-LLM-call syntheses one hourly tick can trigger.
const MAX_GROUPS_PER_TICK: usize = 3;

/// Run one reflection tick: find candidate groups, ask a model to synthesize each, write the
/// grounded ones. Returns how many new reflection facts were written. No-ops entirely under the
/// offline mock provider (mirrors `dissent.rs`/`contradiction.rs`: the offline demo cannot judge
/// this, so it stays silent rather than guessing).
///
/// `warm_age` is the same staleness window the caller passes to `Memory::consolidate` - reflection
/// reuses the exact same "not touched in N days" candidate pool, not a separate one, so it is a
/// parameter here rather than a private constant (the call site owns the sleep-cycle windows, same
/// as `consolidate`/`auto_prune` already do).
pub async fn run_tick(
    memory: &Memory,
    gateway: &Gateway,
    model: &str,
    warm_age: Duration,
) -> usize {
    if gateway.provider_id() == "mock" {
        return 0;
    }
    let groups =
        match memory.reflection_candidates(warm_age, CLUSTER_SIMILARITY, MAX_GROUPS_PER_TICK) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, "reflection_candidates failed");
                return 0;
            }
        };
    let mut written = 0;
    for group in groups {
        match reflect_one(memory, gateway, model, &group).await {
            Ok(true) => written += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, "reflection write failed"),
        }
    }
    written
}

async fn reflect_one(
    memory: &Memory,
    gateway: &Gateway,
    model: &str,
    group: &[Record],
) -> Result<bool, engram_memory::MemoryError> {
    let listing = crate::citation::number_candidates(group.iter().map(|r| r.text.as_str()));
    let prompt = format!(
        "Here are several related memories that might combine into one useful higher-level \
         insight:\n{listing}\nIf they genuinely combine into something new and useful beyond just \
         restating what's already there, reply on ONE line: `REFLECT: <comma-separated numbers of \
         every memory you drew on> | <the synthesized insight, one sentence, stating what those \
         facts together tell you>`. If they don't meaningfully combine into anything beyond what's \
         already stated, reply exactly `NONE`."
    );
    let req = CompletionRequest::new(model.to_string(), vec![Message::user(prompt)]);
    let Ok(out) = gateway
        .complete(Call::new(req).actor("reflection").tainted(Taint::Trusted))
        .await
    else {
        return Ok(false);
    };
    let Some((idxs, insight)) =
        crate::citation::parse_cited_claim(&out.text, "REFLECT:", group.len())
    else {
        return Ok(false);
    };
    // A reflection with no stated synthesis is not grounded - drop it entirely rather than write
    // an empty or citation-only fact (the plan's "drop the output entirely if any claim isn't
    // grounded" rule).
    if insight.is_empty() {
        return Ok(false);
    }
    let sources: Vec<&Record> = idxs.iter().map(|&i| &group[i - 1]).collect();
    let scope = Scope::from_parts(&group[0].scope_kind, &group[0].scope_id);
    let meta = json!({
        "reflection": true,
        "source_ids": sources.iter().map(|s| s.id).collect::<Vec<_>>(),
        "source_seqs": sources.iter().filter_map(|s| s.ledger_seq).collect::<Vec<_>>(),
    });
    memory.remember(
        WriteReq::new(Region::Semantic, insight)
            .source("reflection")
            .actor("core")
            .taint(Taint::Trusted)
            .importance(0.55)
            .scope(scope)
            .tree_level(1)
            .metadata(meta),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::Ledger;
    use engram_gateway::{Completion, MockProvider, ScriptedProvider};
    use engram_memory::TrigramHashEmbedder;

    fn setup(script: Vec<Completion>) -> (Memory, Gateway, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = std::sync::Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Memory::open(
            dir.path().join("b.db"),
            std::sync::Arc::new(TrigramHashEmbedder::default()),
            ledger.clone(),
        )
        .unwrap();
        let gateway = Gateway::new(Box::new(ScriptedProvider::new(script)), ledger);
        (memory, gateway, dir)
    }

    fn completion(text: &str) -> Completion {
        Completion {
            text: text.to_string(),
            model: "test".into(),
            tokens_in: 0,
            tokens_out: 0,
            tool_calls: Vec::new(),
        }
    }

    /// The staleness window passed to every test's `run_tick` call: zero, so a fact becomes a
    /// candidate as soon as any time at all has elapsed since it was written. Callers still sleep a
    /// couple milliseconds after seeding (below) so this never races the millisecond clock.
    const TEST_WARM_AGE: Duration = Duration::from_millis(0);

    async fn seed_related_facts(memory: &Memory) {
        for text in [
            "the payment gateway staging config uses TLS 1.2 certificates",
            "the payment gateway staging config was migrated to TLS 1.3 certificates",
            "the payment gateway staging environment enforces strict TLS certificate checks",
        ] {
            memory
                .remember(WriteReq::new(Region::Semantic, text).importance(0.2))
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    #[tokio::test]
    async fn a_grounded_synthesis_writes_a_trusted_semantic_reflection() {
        let (memory, gateway, _dir) = setup(vec![completion(
            "REFLECT: 1,2,3 | the payment gateway's staging TLS setup was hardened over time",
        )]);
        seed_related_facts(&memory).await;

        let n = run_tick(&memory, &gateway, "test-model", TEST_WARM_AGE).await;
        assert_eq!(n, 1, "one grounded group should write one reflection");

        let hits = memory
            .recall_trusted("payment gateway TLS hardened", &[Region::Semantic], 5)
            .unwrap();
        let reflection = hits
            .iter()
            .find(|h| h.record.metadata.get("reflection") == Some(&serde_json::Value::Bool(true)))
            .expect("a reflection fact must be recallable");
        assert_eq!(reflection.record.tree_level, 1);
        assert_eq!(reflection.record.taint, "trusted");
        assert_eq!(reflection.record.source.as_deref(), Some("reflection"));
        let source_ids = reflection.record.metadata["source_ids"].as_array().unwrap();
        assert_eq!(
            source_ids.len(),
            3,
            "all three cited sources must be recorded"
        );
    }

    #[tokio::test]
    async fn a_none_reply_writes_nothing() {
        let (memory, gateway, _dir) = setup(vec![completion("NONE")]);
        seed_related_facts(&memory).await;

        let n = run_tick(&memory, &gateway, "test-model", TEST_WARM_AGE).await;
        assert_eq!(n, 0, "the model saying NONE must write nothing");
    }

    #[tokio::test]
    async fn a_hallucinated_citation_writes_nothing() {
        // Only 3 candidates exist in the cluster, but the model cites index 9 - fabricated.
        let (memory, gateway, _dir) = setup(vec![completion("REFLECT: 9 | trust me")]);
        seed_related_facts(&memory).await;

        let n = run_tick(&memory, &gateway, "test-model", TEST_WARM_AGE).await;
        assert_eq!(n, 0, "an out-of-range citation must be dropped entirely");
    }

    #[tokio::test]
    async fn a_citation_with_no_synthesis_writes_nothing() {
        let (memory, gateway, _dir) = setup(vec![completion("REFLECT: 1,2")]);
        seed_related_facts(&memory).await;

        let n = run_tick(&memory, &gateway, "test-model", TEST_WARM_AGE).await;
        assert_eq!(
            n, 0,
            "citations with no stated synthesis are not grounded enough to write"
        );
    }

    #[tokio::test]
    async fn no_candidate_group_never_calls_the_model() {
        let (memory, gateway, _dir) = setup(vec![completion("REFLECT: 1 | should never be read")]);
        // A single, unrelated fact never forms a cluster of 2+, so nothing should even be asked.
        memory
            .remember(
                WriteReq::new(Region::Semantic, "the cafeteria menu changes on Tuesdays")
                    .importance(0.2),
            )
            .unwrap();

        let n = run_tick(&memory, &gateway, "test-model", TEST_WARM_AGE).await;
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn the_offline_mock_provider_never_reflects() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = std::sync::Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Memory::open(
            dir.path().join("b.db"),
            std::sync::Arc::new(TrigramHashEmbedder::default()),
            ledger.clone(),
        )
        .unwrap();
        let gateway = Gateway::new(Box::new(MockProvider), ledger);
        seed_related_facts(&memory).await;

        let n = run_tick(&memory, &gateway, "test-model", TEST_WARM_AGE).await;
        assert_eq!(
            n, 0,
            "the offline mock cannot judge a synthesis, so it must stay silent"
        );
    }
}
