//! Auditable dissent - a grounded, cited objection, never prompt-only theatre.
//!
//! Before a task runs, Engram recalls what it knows that bears on it and - ONLY when a real model is
//! connected - asks whether any of those (real, trusted) memories genuinely conflict with the task.
//! The model must CITE the specific memories by number; we keep only citations that map to a memory
//! actually recalled (hallucinated citations are dropped), so the objection's grounds are always
//! real and openable in the brain. If nothing real conflicts - or no model is connected to assess -
//! there is no dissent: silence, not a costume. The raised objection + the user's choice are signed
//! together as one ledger artifact, so even the disagreement is auditable.

use engram_core::{ScopeCtx, Taint};
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use engram_memory::{Memory, Region};
use serde::Serialize;

/// One piece of cited evidence - a real recalled memory the objection rests on.
#[derive(Serialize)]
pub struct Ground {
    pub id: i64,
    pub region: String,
    pub text: String,
}

/// A grounded specialist objection.
#[derive(Serialize)]
pub struct Dissent {
    pub objection: String,
    pub grounds: Vec<Ground>,
}

/// Review a task against recalled memory. Returns a grounded objection, or `None` when nothing real
/// conflicts (or no model is connected to assess - in which case we stay silent rather than guess).
///
/// `scope` bounds the recall to the rings that apply to this task, so a dissent check for a task in
/// project B can never surface (and then DISPLAY as citable grounds) project A's memories — the
/// cross-project bleed this branch exists to stop. Pass the task's session/project scope, or
/// [`ScopeCtx::user_only`] for the unscoped task board (task-board runs are user-global by design).
pub async fn review(
    memory: &Memory,
    gateway: &Gateway,
    model: &str,
    task: &str,
    scope: &ScopeCtx,
) -> Option<Dissent> {
    // Only a real model can assess conflict; the offline mock cannot, so we do not pretend it can.
    if gateway.provider_id() == "mock" {
        return None;
    }
    let regions = [Region::Identity, Region::Semantic, Region::Episodic];
    let hits = memory.recall_trusted_scoped(task, &regions, 8, scope).ok()?;
    if hits.is_empty() {
        return None;
    }
    // (id, region, text) for each recalled memory - the citable, verifiable set.
    let cites: Vec<(i64, String, String)> = hits
        .iter()
        .map(|h| (h.record.id, h.record.region.clone(), h.record.text.clone()))
        .collect();
    // Number them so the model can cite by index and we can verify each citation is real.
    let mut listing = String::new();
    for (i, (_, _, text)) in cites.iter().enumerate() {
        listing.push_str(&format!("{}. {}\n", i + 1, text));
    }
    let prompt = format!(
        "You are a careful specialist reviewing a task against what is known about the user. Identify \
         ONLY facts that genuinely CONFLICT with, or caution against, the task - not facts that merely \
         relate to it. Cite them by number.\n\nTASK: {task}\n\nKNOWN FACTS:\n{listing}\nReply on ONE \
         line. If some facts conflict: `CONFLICT: <comma-separated numbers> | <one short sentence \
         why>`. If none conflict: `NONE`."
    );
    let req = CompletionRequest::new(model.to_string(), vec![Message::user(prompt)]);
    let out = gateway
        .complete(Call::new(req).actor("specialist").tainted(Taint::Trusted))
        .await
        .ok()?;
    parse(&out.text, &cites)
}

/// Parse the specialist's reply, keeping only citations that map to a really-recalled memory.
/// `cites` is the recalled set as `(id, region, text)`, in the order they were numbered.
fn parse(reply: &str, cites: &[(i64, String, String)]) -> Option<Dissent> {
    let line = reply.trim().lines().next()?.trim();
    // Anything that isn't an explicit CONFLICT line (incl. "NONE", or a mock echo) yields no dissent.
    let rest = line.strip_prefix("CONFLICT:")?;
    let (nums, why) = rest.split_once('|').unwrap_or((rest, ""));
    let mut grounds = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tok in nums.split(',') {
        if let Ok(n) = tok.trim().parse::<usize>() {
            // Verify: a 1-based index into the set we actually recalled. Drop anything else.
            if n >= 1 && n <= cites.len() && seen.insert(n) {
                let (id, region, text) = &cites[n - 1];
                grounds.push(Ground {
                    id: *id,
                    region: region.clone(),
                    text: text.clone(),
                });
            }
        }
    }
    if grounds.is_empty() {
        // The model claimed a conflict but cited nothing real - treat as no grounded dissent.
        return None;
    }
    let why = why.trim();
    let objection = if why.is_empty() {
        "This may conflict with what I know about you.".to_string()
    } else {
        why.to_string()
    };
    Some(Dissent { objection, grounds })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cites() -> Vec<(i64, String, String)> {
        vec![
            (10, "identity".into(), "Never deploy on Fridays".into()),
            (
                11,
                "semantic".into(),
                "Prod requires a staging soak first".into(),
            ),
            (12, "identity".into(), "Prefers concise answers".into()),
        ]
    }

    #[test]
    fn none_means_no_dissent() {
        assert!(parse("NONE", &cites()).is_none());
        assert!(parse("nothing conflicts here", &cites()).is_none());
        assert!(parse("", &cites()).is_none());
    }

    #[test]
    fn grounded_conflict_keeps_only_real_citations() {
        // The model cites 1 and 2 (real) and 9 (out of range / hallucinated) - 9 is dropped.
        let d = parse(
            "CONFLICT: 1, 2, 9 | deploying now skips the Friday rule and the soak",
            &cites(),
        )
        .expect("should dissent");
        assert_eq!(d.grounds.len(), 2);
        assert_eq!(d.grounds[0].id, 10);
        assert_eq!(d.grounds[1].id, 11);
        assert!(d.objection.contains("soak"));
    }

    #[test]
    fn conflict_with_only_hallucinated_citations_is_dropped() {
        // Cites nothing real => no grounded dissent (no costume).
        assert!(parse("CONFLICT: 7, 99 | trust me", &cites()).is_none());
    }

    #[test]
    fn deduplicates_repeated_citations() {
        let d = parse("CONFLICT: 1,1,1 | repeated", &cites()).expect("should dissent");
        assert_eq!(d.grounds.len(), 1);
    }

    #[test]
    fn missing_reason_gets_a_default() {
        let d = parse("CONFLICT: 1", &cites()).expect("should dissent");
        assert!(!d.objection.is_empty());
    }
}
