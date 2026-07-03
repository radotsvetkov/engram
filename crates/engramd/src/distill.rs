//! Autonomous skill distillation — the reflection half of the self-improvement loop.
//!
//! After a task finishes, ask the model (only a REAL one — the mock stays silent, never a costume)
//! whether the solution yields a reusable program. It can reply in two shapes:
//!   - a NEW skill (a fresh stdin→stdout program worth keeping), or
//!   - an IMPROVEMENT to an existing skill (`improves: "<id>"` + a better program).
//!
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
    /// Egress the program needs, a subset of {"net","llm"}. A NEW skill that fetches the web declares
    /// "net" so it is installed network-capable; such a skill can't be replay-verified offline, so it
    /// is staged for a one-tap human approval instead of auto-adopting. Empty = a pure transform.
    pub capabilities: Vec<String>,
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
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
struct RawEx {
    #[serde(default)]
    input: String,
    #[serde(default)]
    output: String,
}

/// What a reflection ask produced. Every non-proposal outcome carries WHY, so the caller can sign
/// it into the ledger — a self-improvement loop that drops its failures silently is undebuggable
/// (64 distiller calls once produced zero visible output and nobody could say why).
pub enum ProposeOutcome {
    Proposal(Proposal),
    /// The model was asked and nothing usable came back. `reason` is a stable slug ("none",
    /// "no_json", "bad_json", …); `reply_head` is the start of the visible reply for diagnosis.
    Declined {
        reason: &'static str,
        reply_head: String,
    },
    /// No real model connected (the mock stays silent, never a costume).
    Unavailable,
    /// The gateway call itself failed (auth, rate-limit, network).
    Error(String),
}

/// The model-visible part of a reply: everything after the LAST closing reasoning tag. Reasoning
/// models (minimax, deepseek-r1, qwen) interleave `<think>…</think>` into `message.content`; the
/// braces and stray "NONE"s inside that block wrecked the old prefix/first-brace parser.
fn visible_text(t: &str) -> &str {
    let mut s = t;
    for tag in ["</think>", "</thinking>", "</reasoning>"] {
        if let Some(i) = s.rfind(tag) {
            s = &s[i + tag.len()..];
        }
    }
    s.trim()
}

/// Balanced top-level `{…}` spans in `t`, string-and-escape aware, in order of appearance. The
/// reply may hold prose, code fences, or several objects — each candidate is tried until one
/// deserializes into a usable proposal.
fn json_candidates(t: &str) -> Vec<&str> {
    let bytes = t.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' if depth > 0 => in_str = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    out.push(&t[start..=i]);
                }
            }
            _ => {}
        }
    }
    out
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
/// to an existing one. Shape-validates only (id charset, interpreter charset, non-empty source);
/// resolving the id against the registry (does the target exist? does a new id collide?) is the
/// caller's job, because only the caller can turn a collision into an improvement instead of a drop.
/// `catalog` is a human-readable "id — description" listing for the prompt.
pub async fn propose(
    gateway: &Gateway,
    model: &str,
    task: &str,
    answer: &str,
    catalog: &str,
) -> ProposeOutcome {
    if gateway.provider_id() == "mock" {
        return ProposeOutcome::Unavailable;
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
         \"capabilities\":[],\"examples\":[{{\"input\":\"...\",\"output\":\"...\"}}]}}.\n\
         For an IMPROVEMENT: \
         {{\"improves\":\"<existing id>\",\"interpreter\":\"python3\",\"source\":\"<better program>\",\
         \"description\":\"<one line, optional>\"}}.\n\
         If the program must reach the INTERNET (fetch a URL/API), set \"capabilities\":[\"net\"] — such \
         a skill is staged for one-tap approval, not auto-activated. A pure transform leaves it empty \
         and should include 1-3 examples whose output you are CONFIDENT is correct (the gold it is \
         verified against). Keep the program short and dependency-light."
    );
    // A real proposal is a whole program + gold examples, and reasoning models spend part of the
    // output budget thinking — the 1024-token default truncated proposals MID-JSON (the ledger
    // showed a valid improvement cut off at "def main():", declined as no_json). Give it room.
    let req =
        CompletionRequest::new(model.to_string(), vec![Message::user(prompt)]).max_tokens(8192);
    let out = match gateway
        .complete(Call::new(req).actor("distiller").tainted(Taint::Trusted))
        .await
    {
        Ok(o) => o,
        Err(e) => return ProposeOutcome::Error(e.to_string()),
    };
    match parse(&out.text) {
        Ok(p) => ProposeOutcome::Proposal(p),
        Err((reason, reply_head)) => ProposeOutcome::Declined { reason, reply_head },
    }
}

/// The first chars of the visible reply, for the ledger — enough to diagnose a decline without
/// storing the whole reply.
fn head(t: &str) -> String {
    t.chars().take(240).collect()
}

/// Parse the model's reply into a shape-validated proposal, or say exactly why not. Pure +
/// unit-tested. JSON is preferred over a decline: candidates are tried FIRST, and "NONE" only
/// counts when no usable object exists — a reasoning preamble mentioning "none" must not veto a
/// valid proposal that follows it.
fn parse(reply: &str) -> Result<Proposal, (&'static str, String)> {
    let t = visible_text(reply);
    if t.is_empty() {
        // The whole reply was reasoning (or empty) — nothing visible survived.
        return Err(("empty_reply", head(reply)));
    }
    let candidates = json_candidates(t);
    let had_braces = !candidates.is_empty();
    let mut saw_object = false;
    for cand in candidates {
        let Ok(raw) = serde_json::from_str::<Raw>(cand) else {
            continue;
        };
        saw_object = true;
        let interpreter = if raw.interpreter.trim().is_empty() {
            "python3".to_string()
        } else {
            raw.interpreter.trim().to_string()
        };
        if raw.source.trim().is_empty() {
            continue; // not a proposal object (e.g. an example the model echoed)
        }
        if !valid_interpreter(&interpreter) {
            return Err(("bad_interpreter", head(t)));
        }
        // IMPROVEMENT: targets an existing skill; the caller checks the target exists (and may
        // reinterpret a missing target as a NEW skill, so asserted examples are kept).
        if let Some(target) = raw
            .improves
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !valid_id(target) {
                return Err(("bad_id", head(t)));
            }
            let examples = raw
                .examples
                .into_iter()
                .filter(|e| !e.input.is_empty() || !e.output.is_empty())
                .map(|e| (e.input, e.output))
                .collect();
            return Ok(Proposal {
                id: target.to_string(),
                improves: true,
                interpreter,
                source: raw.source,
                description: raw.description.trim().to_string(),
                when_to_use: raw.when_to_use.filter(|s| !s.trim().is_empty()),
                examples,
                capabilities: Vec::new(), // improvements inherit the existing skill's capabilities
            });
        }
        // NEW skill.
        let id = raw.id.trim().to_string();
        if !valid_id(&id) {
            return Err(("bad_id", head(t)));
        }
        let examples = raw
            .examples
            .into_iter()
            .filter(|e| !e.input.is_empty() || !e.output.is_empty())
            .map(|e| (e.input, e.output))
            .collect();
        // Only egress capabilities the runtime understands survive; anything else is dropped.
        let capabilities = raw
            .capabilities
            .into_iter()
            .map(|c| c.trim().to_ascii_lowercase())
            .filter(|c| c == "net" || c == "llm")
            .collect::<Vec<_>>();
        return Ok(Proposal {
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
            capabilities,
        });
    }
    // No usable object. A "NONE" anywhere in the visible text is the model declining (models
    // rarely emit the bare word we asked for); anything else is a reply we couldn't read.
    if t.to_ascii_lowercase().contains("none") {
        Err(("none", head(t)))
    } else if saw_object {
        Err(("no_source", head(t)))
    } else if had_braces {
        Err(("bad_json", head(t)))
    } else {
        Err(("no_json", head(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_declines_with_reason() {
        assert_eq!(parse("NONE").unwrap_err().0, "none");
        assert_eq!(parse("none, nothing reusable here").unwrap_err().0, "none");
        assert_eq!(parse("").unwrap_err().0, "empty_reply");
    }

    #[test]
    fn valid_json_parses_even_wrapped_in_prose() {
        let reply =
            "Sure, here it is:\n```json\n{\"id\":\"csv_to_json\",\"interpreter\":\"python3\",\
            \"source\":\"import sys,csv,json\\nprint(json.dumps(list(csv.reader(sys.stdin))))\",\
            \"description\":\"convert CSV on stdin to JSON\",\"when_to_use\":\"csv→json\",\
            \"examples\":[{\"input\":\"a,b\",\"output\":\"[[\\\"a\\\",\\\"b\\\"]]\"}]}\n```";
        let p = parse(reply).expect("should parse");
        assert_eq!(p.id, "csv_to_json");
        assert_eq!(p.interpreter, "python3");
        assert_eq!(p.examples.len(), 1);
        assert!(p.source.contains("json.dumps"));
    }

    #[test]
    fn reasoning_block_before_none_still_declines() {
        // What minimax-style reasoning models actually send: thinking (with braces and prose)
        // interleaved into content, then the real answer. The old prefix parser read the think
        // block and never saw the decline.
        let reply = "<think>The task fetched {some} pages… maybe a scraper? No — the output was \
            prose, not a program. I should answer NONE.</think>\nNONE";
        assert_eq!(parse(reply).unwrap_err().0, "none");
    }

    #[test]
    fn reasoning_block_before_json_still_parses() {
        let reply =
            "<think>A reusable {\"id\":\"draft\"} idea… let me write the real one.</think>\n\
            {\"id\":\"line_count\",\"interpreter\":\"python3\",\
            \"source\":\"import sys;print(len(sys.stdin.readlines()))\",\
            \"description\":\"count lines on stdin\"}";
        let p = parse(reply).expect("should parse despite the think block");
        assert_eq!(p.id, "line_count");
    }

    #[test]
    fn a_none_mention_does_not_veto_a_valid_proposal() {
        // JSON wins over the word "none" appearing in surrounding prose.
        let reply = "None of the existing skills cover this, so:\n\
            {\"id\":\"rev\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}";
        assert_eq!(parse(reply).expect("json beats prose-none").id, "rev");
    }

    #[test]
    fn multiple_objects_first_usable_wins() {
        // The model echoes an example object (no source) before the proposal — skip it.
        let reply = "{\"input\":\"a\",\"output\":\"b\"}\n\
            {\"id\":\"ok\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}";
        assert_eq!(parse(reply).expect("second object wins").id, "ok");
    }

    #[test]
    fn unsafe_id_or_interpreter_is_rejected() {
        assert_eq!(
            parse("{\"id\":\"../etc\",\"interpreter\":\"python3\",\"source\":\"x\"}")
                .unwrap_err()
                .0,
            "bad_id"
        );
        assert_eq!(
            parse("{\"id\":\"ok\",\"interpreter\":\"python3; rm -rf /\",\"source\":\"x\"}")
                .unwrap_err()
                .0,
            "bad_interpreter"
        );
    }

    #[test]
    fn empty_source_is_no_source() {
        assert_eq!(
            parse("{\"id\":\"ok\",\"interpreter\":\"python3\",\"source\":\"   \"}")
                .unwrap_err()
                .0,
            "no_source"
        );
    }

    #[test]
    fn unreadable_reply_reasons() {
        assert_eq!(
            parse("I made you a lovely script!").unwrap_err().0,
            "no_json"
        );
        assert_eq!(parse("{\"id\": broken").unwrap_err().0, "no_json"); // unbalanced → no candidate
        assert_eq!(parse("{'id': 'single-quoted'}").unwrap_err().0, "bad_json");
    }

    #[test]
    fn new_skill_has_improves_false() {
        let p =
            parse("{\"id\":\"csv_to_json\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}")
                .expect("new skill should parse");
        assert!(!p.improves);
        assert_eq!(p.id, "csv_to_json");
    }

    #[test]
    fn improvement_carries_the_target_id() {
        let reply = "{\"improves\":\"upcase\",\"interpreter\":\"python3\",\
            \"source\":\"import sys; print(sys.stdin.read().upper())\"}";
        let p = parse(reply).expect("improvement should parse");
        assert!(p.improves);
        assert_eq!(p.id, "upcase"); // the target id, not a new one
        assert!(p.source.contains("upper"));
    }

    #[test]
    fn new_skill_keeps_only_known_capabilities() {
        let reply =
            "{\"id\":\"weather_fetch\",\"interpreter\":\"python3\",\"source\":\"import sys\",\
            \"capabilities\":[\"net\",\"BOGUS\",\"llm\"]}";
        let p = parse(reply).expect("should parse");
        assert!(!p.improves);
        assert_eq!(p.capabilities, vec!["net", "llm"]); // BOGUS dropped, case-normalized
    }

    #[test]
    fn pure_skill_has_no_capabilities() {
        let p =
            parse("{\"id\":\"rev\",\"interpreter\":\"python3\",\"source\":\"print(1)\"}").unwrap();
        assert!(p.capabilities.is_empty());
    }

    #[test]
    fn json_candidates_respects_strings_and_nesting() {
        let t = r#"prose {"a":{"b":"}"}} tail {"c":1}"#;
        let c = json_candidates(t);
        assert_eq!(c, vec![r#"{"a":{"b":"}"}}"#, r#"{"c":1}"#]);
    }
}
