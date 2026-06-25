//! Provider-agnostic request/response shapes. Deliberately minimal — the common
//! denominator across Anthropic, OpenAI, and OpenRouter-style chat + embedding APIs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One message. `secret` marks content that carries keys or private context the
/// model needs but an *untrusted* run must not — the gateway drops these on tainted
/// calls (see [`crate::gateway::Gateway::complete`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default)]
    pub secret: bool,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into(), secret: false }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into(), secret: false }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into(), secret: false }
    }
    /// Mark this message as secret-bearing.
    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self { model: model.into(), messages, max_tokens: 1024, temperature: 0.7 }
    }
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = t;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    pub model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
}
