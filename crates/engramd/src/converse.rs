//! Conversation — where remembering, recalling, and the model of *you* come together.
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

pub async fn converse(memory: &Memory, gateway: &Gateway, text: &str) -> Result<Turn, String> {
    // 1. Record the user's message as a lived experience.
    memory
        .remember(WriteReq::new(Region::Episodic, text).source("user").actor("user"))
        .map_err(|e| e.to_string())?;

    // 2. Recall what we already know that bears on this message.
    let regions = [Region::Identity, Region::Episodic, Region::Semantic];
    let hits = memory.recall(text, &regions, 5).map_err(|e| e.to_string())?;
    let recalled: Vec<String> = hits.iter().map(|h| h.record.text.clone()).collect();

    // 3. Deepen the model of the user from what they just said.
    let learned = extract_identity(text);
    for fact in &learned {
        memory
            .remember(
                WriteReq::new(Region::Identity, fact.clone())
                    .source("inferred")
                    .importance(0.8)
                    .actor("core"),
            )
            .map_err(|e| e.to_string())?;
    }

    // 4. Assemble context and answer through the gateway (mock unless --features http).
    let mut messages = vec![Message::system(
        "You are Engram, a personal agent that remembers the user and grows with them.",
    )];
    if !recalled.is_empty() {
        messages.push(Message::system(format!(
            "What you remember that may be relevant:\n- {}",
            recalled.join("\n- ")
        )));
    }
    messages.push(Message::user(text));
    let req = CompletionRequest::new("claude-haiku", messages);
    let completion = gateway
        .complete(Call::new(req).actor("converse").tainted(Taint::Trusted))
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

/// Cheap, transparent identity extraction. Deliberately simple and auditable — every
/// inferred fact lands in the identity region and the ledger, where it can be seen and
/// forgotten. (A model-based extractor can replace this behind the same write path.)
fn extract_identity(text: &str) -> Vec<String> {
    const RULES: &[(&str, &str)] = &[
        ("i like ", "User likes "),
        ("i love ", "User loves "),
        ("i prefer ", "User prefers "),
        ("i'm ", "User is "),
        ("i am ", "User is "),
        ("my name is ", "User's name is "),
        ("i work ", "User works "),
        ("i live ", "User lives "),
        ("i use ", "User uses "),
    ];
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for (pat, prefix) in RULES {
        if let Some(idx) = lower.find(pat) {
            let rest = &text[idx + pat.len()..];
            let frag = rest.split(['.', '!', '?', '\n', ',']).next().unwrap_or("");
            // Stop at conjunctions so one clause doesn't swallow the next.
            let frag = frag.split(" and ").next().unwrap_or(frag);
            let frag = frag.split(" but ").next().unwrap_or(frag).trim();
            if !frag.is_empty() && frag.len() < 120 {
                out.push(format!("{prefix}{frag}"));
            }
        }
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_identity_facts() {
        let f = extract_identity("Hi, I like Rust and I prefer minimal dependencies.");
        assert!(f.iter().any(|s| s == "User likes Rust"), "got {f:?}");
        assert!(f.iter().any(|s| s == "User prefers minimal dependencies"), "got {f:?}");
    }

    #[test]
    fn extracts_name() {
        let f = extract_identity("my name is Radoslav");
        assert_eq!(f, vec!["User's name is Radoslav".to_string()]);
    }

    #[test]
    fn nothing_to_extract() {
        assert!(extract_identity("what time is it?").is_empty());
    }
}
