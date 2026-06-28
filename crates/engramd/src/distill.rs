//! Autonomous skill distillation — OFF by default.
//!
//! After a task finishes, ask the model (only a REAL one — the mock stays silent, never a costume)
//! whether the solution contains a single, general, reusable program worth keeping as a skill. If so
//! it proposes one; the daemon installs it **inactive** (it must earn activation), signed and
//! ledgered as `skill.distill`. This is what makes skills pop up WITHOUT the agent being prompted —
//! gated behind a config flag and a tool-step threshold so a daemon that opts out pays nothing.

use engram_core::Taint;
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use serde::Deserialize;

/// A proposed reusable skill distilled from a finished task.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub id: String,
    pub interpreter: String,
    pub source: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub examples: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Raw {
    id: String,
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

/// Ask the model whether the finished task yields a reusable program. `None` when no real model is
/// connected, the model declines, or the proposal is invalid / a duplicate.
pub async fn propose(
    gateway: &Gateway,
    model: &str,
    task: &str,
    answer: &str,
    existing: &[String],
) -> Option<Proposal> {
    if gateway.provider_id() == "mock" {
        return None;
    }
    let existing_list = if existing.is_empty() {
        "(none)".to_string()
    } else {
        existing.join(", ")
    };
    let answer: String = answer.chars().take(2000).collect();
    let prompt = format!(
        "You just finished a task. Decide whether the SOLUTION contains a single, GENERAL, reusable \
         program worth keeping as a skill for future tasks — a small script that reads its input on \
         stdin and writes its result to stdout. Only propose one if it would genuinely be reused \
         across tasks and is not a throwaway one-off. Existing skills (do NOT duplicate or rename): \
         {existing_list}.\n\nTASK: {task}\n\nRESULT: {answer}\n\nReply with EITHER the single word \
         NONE, or a single JSON object (no prose, no code fences): \
         {{\"id\":\"snake_case_slug\",\"interpreter\":\"python3\",\"source\":\"<the program>\",\
         \"description\":\"<one line>\",\"when_to_use\":\"<short cue>\",\
         \"examples\":[{{\"input\":\"...\",\"output\":\"...\"}}]}}. Keep the program short and \
         dependency-light; include 1-3 examples whose output you are confident is correct."
    );
    let req = CompletionRequest::new(model.to_string(), vec![Message::user(prompt)]);
    let out = gateway
        .complete(Call::new(req).actor("distiller").tainted(Taint::Trusted))
        .await
        .ok()?;
    parse(&out.text, existing)
}

/// Parse the model's reply into a validated proposal. Pure + unit-tested. Drops anything that isn't
/// a valid, non-duplicate skill (so a hallucinated or malformed reply yields nothing).
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
    let id = raw.id.trim().to_string();
    if !valid_id(&id) || existing.iter().any(|e| e == &id) {
        return None;
    }
    let interpreter = if raw.interpreter.trim().is_empty() {
        "python3".to_string()
    } else {
        raw.interpreter.trim().to_string()
    };
    if !valid_interpreter(&interpreter) || raw.source.trim().is_empty() {
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
}
