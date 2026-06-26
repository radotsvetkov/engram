//! Conversation - where remembering, recalling, and the model of *you* come together.
//!
//! Each turn: the user's message is written to **episodic** memory, relevant past
//! memories are recalled across identity / episodic / semantic regions (this is the
//! "searches its own past conversations" capability), light identity facts are
//! extracted and stored in the **identity** region (the "deepening model of who you
//! are"), the recalled context is assembled into the model prompt through the gateway,
//! and the reply is written back to episodic memory. All of it persists in the SQLite
//! brain, so it survives the core sleeping to zero and carries across sessions.

use engram_core::Taint;
use engram_gateway::{Call, CompletionRequest, Gateway, Message};
use engram_memory::{Memory, Region, WriteReq};

pub struct Turn {
    pub reply: String,
    pub recalled: Vec<String>,
    pub learned: Vec<String>,
}

pub async fn converse(
    memory: &Memory,
    gateway: &Gateway,
    text: &str,
    model: &str,
    persona: Option<&str>,
) -> Result<Turn, String> {
    // The non-streaming path is the streaming one with a sink that discards fragments.
    let mut sink = |_: String| {};
    converse_stream(memory, gateway, text, model, persona, &mut sink).await
}

/// Streaming conversation: identical recall / identity-learning / persistence, but the
/// model's reply is streamed fragment-by-fragment to `on_delta` as it generates, and the
/// assembled [`Turn`] is returned at the end.
pub async fn converse_stream(
    memory: &Memory,
    gateway: &Gateway,
    text: &str,
    model: &str,
    persona: Option<&str>,
    on_delta: &mut (dyn FnMut(String) + Send),
) -> Result<Turn, String> {
    // 1. Record the user's message as a lived experience.
    memory
        .remember(WriteReq::new(Region::Episodic, text).source("user").actor("user"))
        .map_err(|e| e.to_string())?;

    // 2. Recall what we already know that bears on this message.
    let regions = [Region::Identity, Region::Episodic, Region::Semantic];
    // Trusted context only: content the agent read from untrusted sources is stored with
    // its provenance but never re-surfaces here as trusted memory (memory-poisoning guard).
    let hits = memory.recall_trusted(text, &regions, 5).map_err(|e| e.to_string())?;
    let recalled: Vec<String> = hits.iter().map(|h| h.record.text.clone()).collect();

    // 3. Deepen the model of the user from what they just said. A changed *singular*
    //    attribute (name, where they live/work) supersedes the prior value - the old fact
    //    becomes history (kept, ledgered) and stops surfacing, so recall isn't confidently
    //    wrong. Additive facts (likes, uses) accumulate.
    let learned = extract_identity(text);
    for l in &learned {
        let rec = memory
            .remember(
                WriteReq::new(Region::Identity, l.fact.clone())
                    .source("inferred")
                    .importance(0.8)
                    .actor("core"),
            )
            .map_err(|e| e.to_string())?;
        if l.supersede {
            for old in
                memory.current_with_prefix(Region::Identity, &l.prefix).map_err(|e| e.to_string())?
            {
                if old != rec.id {
                    let _ = memory.supersede(old, rec.id);
                }
            }
        }
    }
    let learned: Vec<String> = learned.into_iter().map(|l| l.fact).collect();

    // 4. Assemble context and answer through the gateway (mock unless --features http).
    let mut messages = vec![Message::system(
        "You are Engram, a personal agent that remembers the user and grows with them.",
    )];
    // The active project's standing instructions, if any - this is what gives each project its
    // own voice and priorities.
    if let Some(p) = persona {
        if !p.trim().is_empty() {
            messages.push(Message::system(p.to_string()));
        }
    }
    if !recalled.is_empty() {
        messages.push(Message::system(format!(
            "What you remember that may be relevant:\n- {}",
            recalled.join("\n- ")
        )));
    }
    messages.push(Message::user(text));
    let req = CompletionRequest::new(model.to_string(), messages);
    let completion = gateway
        .complete_stream(Call::new(req).actor("converse").tainted(Taint::Trusted), on_delta)
        .await
        .map_err(|e| e.to_string())?;

    // 5. Remember our own reply, so the conversation is searchable later.
    memory
        .remember(
            WriteReq::new(Region::Episodic, format!("assistant said: {}", completion.text))
                .source("assistant")
                .actor("core"),
        )
        .map_err(|e| e.to_string())?;

    Ok(Turn { reply: completion.text, recalled, learned })
}

/// An inferred identity fact, with the prefix it was derived from and whether it is a
/// *singular* attribute (a new value supersedes the old) or additive (it accumulates).
#[derive(Debug)]
struct Learned {
    prefix: String,
    fact: String,
    supersede: bool,
}

/// Cheap, transparent identity extraction. Deliberately simple and auditable - every
/// inferred fact lands in the identity region and the ledger, where it can be seen and
/// forgotten. Singular attributes (name, where you live/work, who you are) supersede the
/// prior value; preferences (like/love/prefer/use) accumulate. (A model-based extractor can
/// replace this behind the same write path.)
fn extract_identity(text: &str) -> Vec<Learned> {
    // (pattern, output prefix, supersede-prior?). Supersede is reserved for genuinely
    // singular attributes whose output prefix is *unique and unambiguous* - name, where
    // you live, where you work. "i'm/i am" → "User is " is deliberately NOT superseding:
    // its prefix is a generic catch-all ("User is happy", "User is a developer", "User is
    // tired" all share it), so superseding on it would let a passing mood bury a durable
    // fact. State-of-being and preferences accumulate; a richer attribute-keyed model can
    // make them singular later.
    const RULES: &[(&str, &str, bool)] = &[
        ("i like ", "User likes ", false),
        ("i love ", "User loves ", false),
        ("i prefer ", "User prefers ", false),
        ("i use ", "User uses ", false),
        ("i'm ", "User is ", false),
        ("i am ", "User is ", false),
        ("my name is ", "User's name is ", true),
        ("i work ", "User works ", true),
        ("i live ", "User lives ", true),
    ];
    let lower = text.to_lowercase();
    let mut out: Vec<Learned> = Vec::new();
    for (pat, prefix, supersede) in RULES {
        if let Some(idx) = lower.find(pat) {
            let rest = &text[idx + pat.len()..];
            let frag = rest.split(['.', '!', '?', '\n', ',']).next().unwrap_or("");
            // Stop at conjunctions so one clause doesn't swallow the next.
            let frag = frag.split(" and ").next().unwrap_or(frag);
            let frag = frag.split(" but ").next().unwrap_or(frag).trim();
            if !frag.is_empty() && frag.len() < 120 {
                let fact = format!("{prefix}{frag}");
                if !out.iter().any(|l| l.fact == fact) {
                    out.push(Learned { prefix: prefix.to_string(), fact, supersede: *supersede });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_identity_facts() {
        let f = extract_identity("Hi, I like Rust and I prefer minimal dependencies.");
        assert!(f.iter().any(|l| l.fact == "User likes Rust"), "got {f:?}");
        assert!(f.iter().any(|l| l.fact == "User prefers minimal dependencies"), "got {f:?}");
        // Preferences are additive, not superseding.
        assert!(f.iter().all(|l| !l.supersede));
    }

    #[test]
    fn extracts_name_as_a_superseding_singular() {
        let f = extract_identity("my name is Radoslav");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].fact, "User's name is Radoslav");
        assert!(f[0].supersede, "name is a singular attribute that supersedes the prior value");
        assert_eq!(f[0].prefix, "User's name is ");
    }

    #[test]
    fn state_of_being_is_additive_not_superseding() {
        // "I am ..." → "User is ..." must NOT supersede: its prefix is a generic catch-all,
        // so a passing mood ("User is tired") must never bury a durable fact.
        let f = extract_identity("I am a developer");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].fact, "User is a developer");
        assert!(!f[0].supersede);
    }

    #[test]
    fn nothing_to_extract() {
        assert!(extract_identity("what time is it?").is_empty());
    }
}
