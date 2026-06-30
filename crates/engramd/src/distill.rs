//! Autonomous skill distillation — the reflection half of the self-improvement loop.
//!
//! After a task finishes, ask the model (only a REAL one — the mock stays silent, never a costume)
//! whether the solution yields a reusable program. It can reply in two shapes:
//!   - a NEW skill (a fresh stdin→stdout program worth keeping), or
//!   - an IMPROVEMENT to an existing skill (`improves: "<id>"` + a better program).
//! This runs as a SEPARATE `Trusted` model call (it never streams raw tool output into the authoring
//! decision), so it is safe to run even after a tainted (web) task — nothing it proposes becomes
//! active until it earns activation through the verification gate in `engram-agent` (replay against
//! gold, sandboxed, capability-clamped). New skills are installed **inactive**; the only thing that
//! flips a skill active is passing that gate. Gated behind a config flag and a tool-step threshold
//! so a daemon that opts out pays nothing.

use engram_core::Taint;
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use serde::Deserialize;

/// A proposed skill distilled from a finished task — either a new program or an improvement to an
/// existing one.
#[derive(Debug, Clone)]
pub struct Proposal {
    /// For a new skill: its fresh id. For an improvement: the id of the EXISTING skill it replaces.
    pub id: String,
    /// True when this is an improvement to the existing skill named by `id`; false for a new skill.
    pub improves: bool,
    pub interpreter: String,
    pub source: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub examples: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    id: String,
    /// When set to an existing skill id, the proposal is an IMPROVEMENT to that skill (and `id` is
    /// ignored in favor of this target).
    #[serde(default)]
    improves: Option<String>,
    #[serde(default)]
    interpreter: String,
    source: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    when_to_use: Option<String>,
    #[serde(default)]
    examples: Vec<RawEx>,
}

#[derive(Deserialize)]
struct RawEx {
    #[serde(default)]
    input: String,
    #[serde(default)]
    output: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_interpreter(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '/' | '.' | '_' | '-'))
}

/// Ask the model whether the finished task yields a reusable program — a NEW skill or an IMPROVEMENT
/// to an existing one. `None` when no real model is connected, the model declines, or the proposal is
/// invalid. `existing` is the id set used to validate (improvement targets must exist; new ids must
/// not collide); `catalog` is a human-readable "id — description" listing for the prompt.
pub async fn propose(
    gateway: &Gateway,
    model: &str,
    task: &str,
    answer: &str,
    existing: &[String],
    catalog: &str,
) -> Option<Proposal> {
    if gateway.provider_id() == "mock" {
        return None;
    }
    let catalog = if catalog.trim().is_empty() {
        "(none)".to_string()
    } else {
        catalog.trim().to_string()
    };
    let answer: String = answer.chars().take(2000).collect();
    let prompt = format!(
        "You just finished a task. Decide whether the SOLUTION yields a single, GENERAL, reusable \
         program worth keeping — a small script that reads its input on stdin and writes its result \
         to stdout. You have THREE choices:\n\
         1. NONE — the work was a throwaway one-off, nothing reusable.\n\
         2. A NEW skill — a fresh program for a job none of the existing skills do.\n\
         3. An IMPROVEMENT — one of the existing skills does this job but imperfectly; offer a better \
         program for it by setting \"improves\" to its exact id.\n\n\
         EXISTING SKILLS (id — what it does):\n{catalog}\n\n\
         TASK: {task}\n\nRESULT: {answer}\n\n\
         Reply with EITHER the single word NONE, or a single JSON object (no prose, no code fences).\n\
         For a NEW skill: \
         {{\"id\":\"snake_case_slug\",\"interpreter\":\"python3\",\"source\":\"<the program>\",\
         \"description\":\"<one line>\",\"when_to_use\":\"<short cue>\",\
         \"examples\":[{{\"input\":\"...\",\"output\":\"...\"}}]}}.\n\
         For an IMPROVEMENT: \
         {{\"improves\":\"<existing id>\",\"interpreter\":\"python3\",\"source\":\"<better program>\",\
         \"description\":\"<one line, optional>\"}}.\n\
         Keep the program short and dependency-light; for a new skill include 1-3 examples whose \
         output you are CONFIDENT is correct (they become the gold the skill is verified against)."
    );
    let req = CompletionRequest::new(model.to_string(), vec![Message::user(prompt)]);
    let out = gateway
        .complete(Call::new(req).actor("distiller").tainted(Taint::Trusted))
        .await
        .ok()?;
    parse(&out.text, existing)
}

/// Parse the model's reply into a validated proposal. Pure + unit-tested. Drops anything that isn't a
/// valid new skill (non-duplicate id) or improvement (existing target). A hallucinated or malformed
/// reply yields nothing.
fn parse(reply: &str, existing: &[String]) -> Option<Proposal> {
    let t = reply.trim();
    if t.is_empty() || t.starts_with("NONE") || t.starts_with("none") {
        return None;
    }
    // Extract the first {...} object (the model may wrap it in fences or stray prose).
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end <= start {
        return None;
    }
    let raw: Raw = serde_json::from_str(&t[start..=end]).ok()?;
    let interpreter = if raw.interpreter.trim().is_empty() {
        "python3".to_string()
    } else {
        raw.interpreter.trim().to_string()
    };
    if !valid_interpreter(&interpreter) || raw.source.trim().is_empty() {
        return None;
    }
    // IMPROVEMENT: targets an existing skill. The target must exist; `id` is ignored.
    if let Some(target) = raw.improves.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if !existing.iter().any(|e| e == target) {
            return None; // can't improve a skill that doesn't exist
        }
        return Some(Proposal {
            id: target.to_string(),
            improves: true,
            interpreter,
            source: raw.source,
            description: raw.description.trim().to_string(),
            when_to_use: raw.when_to_use.filter(|s| !s.trim().is_empty()),
            examples: Vec::new(),
        });
    }
    // NEW skill: fresh, non-duplicate id.
    let id = raw.id.trim().to_string();
    if !valid_id(&id) || existing.iter().any(|e| e == &id) {
        return None;
    }
    let examples = raw
        .examples
        .into_iter()
        .filter(|e| !e.input.is_empty() || !e.output.is_empty())
        .map(|e| (e.input, e.output))
        .collect();
    Some(Proposal {
        id,
        improves: false,
        interpreter,
        source: raw.source,
        description: if raw.description.trim().is_empty() {
            "(distilled skill)".to_string()
        } else {
            raw.description.trim().to_string()
        },
        when_to_use: raw.when_to_use.filter(|s| !s.trim().is_empty()),
        examples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_yields_nothing() {
        assert!(parse("NONE", &[]).is_none());
        assert!(parse("none, nothing reusable here", &[]).is_none());
        assert!(parse("", &[]).is_none());
    }

    #[test]
    fn valid_json_parses_even_wrapped_in_prose() {
        let reply = "Sure, here it is:\n```json\n{\"id\":\"csv_to_json\",\"interpreter\":\"python3\",\
            \"source\":\"import sys,csv,json\\nprint(json.dumps(list(csv.reader(sys.stdin))))\",\
            \"description\":\"convert CSV on stdin to JSON\",\"when_to_use\":\"csv→json\",\
            \"examples\":[{\"input\":\"a,b\",\"output\":\"[[\\\"a\\\",\\\"b\\\"]]\"}]}\n```";
        let p = parse(reply, &[]).expect("should parse");
        assert_eq!(p.id, "csv_to_json");
        assert_eq!(p.interpreter, "python3");
        assert_eq!(p.examples.len(), 1);
        assert!(p.source.contains("json.dumps"));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let reply = "{\"id\":\"shout\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}";
        assert!(parse(reply, &["shout".to_string()]).is_none());
    }

    #[test]
    fn unsafe_id_or_interpreter_is_rejected() {
        assert!(parse(
            "{\"id\":\"../etc\",\"interpreter\":\"python3\",\"source\":\"x\"}",
            &[]
        )
        .is_none());
        assert!(parse(
            "{\"id\":\"ok\",\"interpreter\":\"python3; rm -rf /\",\"source\":\"x\"}",
            &[]
        )
        .is_none());
    }

    #[test]
    fn empty_source_is_rejected() {
        assert!(parse(
            "{\"id\":\"ok\",\"interpreter\":\"python3\",\"source\":\"   \"}",
            &[]
        )
        .is_none());
    }

    #[test]
    fn new_skill_has_improves_false() {
        let p = parse(
            "{\"id\":\"csv_to_json\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}",
            &[],
        )
        .expect("new skill should parse");
        assert!(!p.improves);
        assert_eq!(p.id, "csv_to_json");
    }

    #[test]
    fn improvement_targets_an_existing_skill() {
        let reply = "{\"improves\":\"upcase\",\"interpreter\":\"python3\",\
            \"source\":\"import sys; print(sys.stdin.read().upper())\"}";
        let p = parse(reply, &["upcase".to_string()]).expect("improvement should parse");
        assert!(p.improves);
        assert_eq!(p.id, "upcase"); // the target id, not a new one
        assert!(p.source.contains("upper"));
    }

    #[test]
    fn improvement_of_unknown_skill_is_rejected() {
        let reply = "{\"improves\":\"nope\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}";
        assert!(parse(reply, &["upcase".to_string()]).is_none());
    }
}
