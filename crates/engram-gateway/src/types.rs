//! Provider-agnostic request/response shapes. The common denominator across Anthropic,
//! OpenAI, and OpenRouter-style chat + embedding APIs — including **tool calling**,
//! which is what turns the gateway from a text oracle into the engine of an agent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    /// A tool result fed back to the model (carries `tool_call_id`).
    Tool,
}

/// A tool the model may call: a name, a description, and a JSON-Schema for its args.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool invocation the model emitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// One message. `secret` marks content the gateway drops on an untrusted call. Tool
/// plumbing rides along: an assistant turn may carry `tool_calls`, and a tool result
/// carries the `tool_call_id` it answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Base64-encoded PNG images attached to this message (for vision).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl Message {
    fn base(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            secret: false,
            tool_call_id: None,
            tool_calls: Vec::new(),
            images: Vec::new(),
        }
    }

    /// A user message with an attached image (base64 PNG) for vision models.
    pub fn user_with_image(content: impl Into<String>, image_b64: impl Into<String>) -> Self {
        let mut m = Self::base(Role::User, content);
        m.images.push(image_b64.into());
        m
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self::base(Role::System, content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::base(Role::User, content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::base(Role::Assistant, content)
    }
    /// An assistant turn that requested tool calls.
    pub fn assistant_tool_calls(content: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        let mut m = Self::base(Role::Assistant, content);
        m.tool_calls = calls;
        m
    }
    /// A tool result answering `tool_call_id`.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        let mut m = Self::base(Role::Tool, content);
        m.tool_call_id = Some(tool_call_id.into());
        m
    }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self { model: model.into(), messages, max_tokens: 1024, temperature: 0.7, tools: Vec::new() }
    }
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = t;
        self
    }
    pub fn tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    pub model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    /// Tool calls the model requested this turn (empty when it answered directly).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}
