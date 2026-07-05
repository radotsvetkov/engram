//! Memory-vs-memory contradiction detection - extends supersession beyond `converse.rs`'s 3-rule
//! literal-prefix whitelist (`RULES`, Identity-region-only, exact-string-prefix matching) to any
//! region and any wording, by asking a model whether a new fact contradicts an existing one it
//! resembles. Reuses `dissent.rs`'s exact citation-and-strip-hallucination discipline
//! (`crate::citation`) so this inherits the same anti-hallucination guarantee instead of a second,
//! weaker one, and NEVER applies a detected contradiction automatically - it only ever proposes one
//! (`Memory::propose_supersession`) for a human to confirm or reject
//! (`Memory::resolve_supersession`). Mandatory confirmation, no silent-auto-apply mode: an opt-in
//! "just apply it" escape hatch would reintroduce the exact unverifiable-silent-overwrite failure
//! this feature exists to fix (see docs/MEMORY-UPGRADE-PLAN.md §5's locked decision).
//!
//! The citations here prove "the model looked at these specific rows" - NOT "the model is correct
//! that they conflict" (unlike `dissent.rs`, which grounds in a hard, checkable replay-win score).
//! Confirmation-surface copy must read as "possible conflict, your call", never as an assertion
//! with dissent's evidentiary weight.

use engram_core::{Scope, Taint};
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use engram_memory::{Memory, Region};

/// How close an existing fact must be (cosine similarity) to even ask a model about it - well
/// above typical unrelated-fact similarity, so this doesn't fire on every ordinary write.
const MIN_SIMILARITY: f32 = 0.55;
/// A candidate LISTING size, not a recall result set - keep it small and cheap to judge.
const MAX_CANDIDATES: usize = 5;

/// Check whether `new_text` (about to be written as `region` in `scope`) contradicts or updates an
/// existing fact closely enough to warrant a human's confirmation. Returns `Some((old_id,
/// pending_id))` when a proposal was recorded; `None` when nothing similar enough exists, the model
/// found no real conflict, or no real model is connected to judge (mirrors `dissent.rs`: the
/// offline mock cannot assess this, so the function stays silent rather than guessing).
pub async fn check(
    memory: &Memory,
    gateway: &Gateway,
    model: &str,
    region: Region,
    scope: &Scope,
    new_text: &str,
    actor: &str,
) -> Option<(i64, i64)> {
    if gateway.provider_id() == "mock" {
        return None;
    }
    let candidates =
        memory.find_similar_not_identical(region, scope, new_text, MIN_SIMILARITY, MAX_CANDIDATES).ok()?;
    if candidates.is_empty() {
        return None;
    }
    let listing = crate::citation::number_candidates(candidates.iter().map(|c| c.text.as_str()));
    let prompt = format!(
        "A new fact is about to be remembered: \"{new_text}\"\n\nHere is what is currently stored \
         that might be the SAME fact stated differently, or might directly CONTRADICT it (not \
         merely related to it):\n{listing}\nDoes the new fact contradict or update any of these? \
         Cite the ones it replaces.\n\nReply on ONE line. If it replaces one or more: \
         `SUPERSEDES: <comma-separated numbers> | <one short reason>`. If it is unrelated, or adds \
         new information without contradicting anything listed: `NONE`."
    );
    let req = CompletionRequest::new(model.to_string(), vec![Message::user(prompt)]);
    let out = gateway
        .complete(Call::new(req).actor("contradiction_detector").tainted(Taint::Trusted))
        .await
        .ok()?;
    let (idxs, reason) =
        crate::citation::parse_cited_claim(&out.text, "SUPERSEDES:", candidates.len())?;
    // One new fact proposes against one old one; if the model cites several, the strongest
    // (highest-similarity, i.e. first-listed) candidate wins - the rest were offered as context,
    // not independently confirmed targets.
    let old = &candidates[idxs[0] - 1];
    let pid = memory
        .propose_supersession(old.id, new_text, &reason, region, scope, actor)
        .ok()?;
    Some((old.id, pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::{Ledger, ScopeCtx};
    use engram_gateway::{Completion, MockProvider, ScriptedProvider};
    use engram_memory::{Memory, TrigramHashEmbedder, WriteReq};

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

    #[tokio::test]
    async fn a_cited_conflict_proposes_a_pending_supersession() {
        let (memory, gateway, _dir) = setup(vec![completion("SUPERSEDES: 1 | the domain moved")]);
        memory
            .remember(WriteReq::new(Region::Semantic, "the API base url is api.old.example"))
            .unwrap();

        let result = check(
            &memory,
            &gateway,
            "test-model",
            Region::Semantic,
            &Scope::user(),
            "the API base url is api.new.example",
            "core",
        )
        .await;
        assert!(result.is_some(), "a genuinely similar, model-confirmed conflict must propose");

        // Nothing was silently applied - the original fact is untouched, only a pending row exists.
        let hits = memory
            .recall_scoped(
                "api base url",
                &[Region::Semantic],
                5,
                &ScopeCtx::user_only(),
            )
            .unwrap();
        assert_eq!(hits.len(), 1, "the candidate fact must not be written until confirmed");
        assert!(hits[0].record.text.contains("old.example"));
        assert_eq!(memory.pending_supersessions().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn no_similar_candidate_never_calls_the_model_or_proposes() {
        let (memory, gateway, _dir) = setup(vec![completion("SUPERSEDES: 1 | should never be read")]);
        memory
            .remember(WriteReq::new(Region::Semantic, "the cafeteria menu changes on Tuesdays"))
            .unwrap();

        let result = check(
            &memory,
            &gateway,
            "test-model",
            Region::Semantic,
            &Scope::user(),
            "the deploy pipeline runs on port 9090",
            "core",
        )
        .await;
        assert!(result.is_none(), "an unrelated fact must never reach the model at all");
        assert!(memory.pending_supersessions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_none_reply_proposes_nothing() {
        let (memory, gateway, _dir) = setup(vec![completion("NONE")]);
        memory
            .remember(WriteReq::new(Region::Semantic, "the API base url is api.old.example"))
            .unwrap();

        let result = check(
            &memory,
            &gateway,
            "test-model",
            Region::Semantic,
            &Scope::user(),
            "the API base url is also documented in the wiki",
            "core",
        )
        .await;
        assert!(result.is_none(), "the model saying NONE must propose nothing");
        assert!(memory.pending_supersessions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_hallucinated_citation_proposes_nothing() {
        // Only 1 candidate exists, but the model cites index 7 - entirely fabricated.
        let (memory, gateway, _dir) = setup(vec![completion("SUPERSEDES: 7 | trust me")]);
        memory
            .remember(WriteReq::new(Region::Semantic, "the API base url is api.old.example"))
            .unwrap();

        let result = check(
            &memory,
            &gateway,
            "test-model",
            Region::Semantic,
            &Scope::user(),
            "the API base url is api.new.example",
            "core",
        )
        .await;
        assert!(result.is_none(), "a citation to a candidate that was never offered must be dropped");
        assert!(memory.pending_supersessions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_offline_mock_provider_never_proposes() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = std::sync::Arc::new(Ledger::open(dir.path()).unwrap());
        let memory = Memory::open(
            dir.path().join("b.db"),
            std::sync::Arc::new(TrigramHashEmbedder::default()),
            ledger.clone(),
        )
        .unwrap();
        let gateway = Gateway::new(Box::new(MockProvider), ledger);
        memory
            .remember(WriteReq::new(Region::Semantic, "the API base url is api.old.example"))
            .unwrap();

        let result = check(
            &memory,
            &gateway,
            "test-model",
            Region::Semantic,
            &Scope::user(),
            "the API base url is api.new.example",
            "core",
        )
        .await;
        assert!(result.is_none(), "the offline mock cannot assess conflict, so it must stay silent");
    }
}
