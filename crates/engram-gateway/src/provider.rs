//! Providers: the concrete backends behind the gateway. The gateway depends only on
//! the [`Provider`] trait, so swapping Anthropic for OpenAI for a local model is a
//! constructor change, never a rewrite.

use async_trait::async_trait;

use crate::types::{Completion, CompletionRequest, Role};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("provider error: {0}")]
    Provider(String),
    #[error("ledger: {0}")]
    Ledger(#[from] engram_core::LedgerError),
}

/// A model backend. Object-safe via `async_trait` so the gateway can hold a
/// `Box<dyn Provider>`.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, GatewayError>;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, GatewayError>;
    /// Short stable id for the audit trail, e.g. "mock", "anthropic", "openai".
    fn id(&self) -> &str;
}

/// Rough token estimate (~4 chars/token). Real providers return exact counts; this
/// is for metering when they don't and for the offline mock.
pub fn approx_tokens(text: &str) -> u32 {
    ((text.chars().count() as f32) / 4.0).ceil() as u32
}

/// A deterministic, offline provider. It never makes a network call, so the whole
/// gateway — metering, taint redaction, audit — is testable without credentials.
pub struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, GatewayError> {
        let tokens_in = req.messages.iter().map(|m| approx_tokens(&m.content)).sum();
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let text = format!("[mock:{}] ack: {}", req.model, first_words(last_user, 12));
        let tokens_out = approx_tokens(&text);
        Ok(Completion { text, model: req.model.clone(), tokens_in, tokens_out, tool_calls: Vec::new() })
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, GatewayError> {
        Ok(texts.iter().map(|t| mock_vec(t)).collect())
    }

    fn id(&self) -> &str {
        "mock"
    }
}

/// A provider that replays a scripted sequence of completions — the way to drive the
/// agent loop deterministically in tests (e.g. "first emit this tool call, then this
/// final answer") without a live model.
pub struct ScriptedProvider {
    queue: std::sync::Mutex<std::collections::VecDeque<Completion>>,
}

impl ScriptedProvider {
    pub fn new(script: Vec<Completion>) -> Self {
        Self { queue: std::sync::Mutex::new(script.into()) }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn complete(&self, req: &CompletionRequest) -> Result<Completion, GatewayError> {
        let next = self.queue.lock().expect("scripted provider mutex").pop_front();
        Ok(next.unwrap_or(Completion {
            text: "done".into(),
            model: req.model.clone(),
            tokens_in: 0,
            tokens_out: 1,
            tool_calls: Vec::new(),
        }))
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, GatewayError> {
        Ok(texts.iter().map(|t| mock_vec(t)).collect())
    }
    fn id(&self) -> &str {
        "scripted"
    }
}

fn first_words(s: &str, n: usize) -> String {
    s.split_whitespace().take(n).collect::<Vec<_>>().join(" ")
}

/// A tiny deterministic 8-dim vector — enough to exercise the embed path offline.
fn mock_vec(text: &str) -> Vec<f32> {
    let mut v = [0f32; 8];
    for (i, b) in text.bytes().enumerate() {
        v[i % 8] += b as f32;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v.to_vec()
}

#[cfg(feature = "http")]
pub use http::HttpProvider;

#[cfg(feature = "http")]
mod http {
    //! An OpenAI-compatible HTTP provider (chat completions + embeddings + tool calling).
    //! Works with OpenAI, OpenRouter, and any compatible gateway by setting `base_url`.
    //! Compiled only with `--features http` so offline builds stay small.

    use super::*;
    use crate::types::{Message, ToolCall};

    pub struct HttpProvider {
        client: reqwest::Client,
        base_url: String,
        api_key: String,
        id: String,
    }

    impl HttpProvider {
        pub fn new(id: impl Into<String>, base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                base_url: base_url.into().trim_end_matches('/').to_string(),
                api_key: api_key.into(),
                id: id.into(),
            }
        }
    }

    fn role_str(r: Role) -> &'static str {
        match r {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    fn message_json(m: &Message) -> serde_json::Value {
        let mut o = serde_json::json!({ "role": role_str(m.role), "content": m.content });
        if let Some(id) = &m.tool_call_id {
            o["tool_call_id"] = serde_json::json!(id);
        }
        if !m.tool_calls.is_empty() {
            o["tool_calls"] = serde_json::Value::Array(
                m.tool_calls
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "type": "function",
                            "function": { "name": c.name, "arguments": c.arguments.to_string() },
                        })
                    })
                    .collect(),
            );
        }
        o
    }

    #[async_trait]
    impl Provider for HttpProvider {
        async fn complete(&self, req: &CompletionRequest) -> Result<Completion, GatewayError> {
            let mut body = serde_json::json!({
                "model": req.model,
                "max_tokens": req.max_tokens,
                "temperature": req.temperature,
                "messages": req.messages.iter().map(message_json).collect::<Vec<_>>(),
            });
            if !req.tools.is_empty() {
                body["tools"] = serde_json::Value::Array(
                    req.tools
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "type": "function",
                                "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
                            })
                        })
                        .collect(),
                );
            }
            let resp = self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let status = resp.status();
            let json: serde_json::Value =
                resp.json().await.map_err(|e| GatewayError::Provider(e.to_string()))?;
            if !status.is_success() {
                return Err(GatewayError::Provider(format!("{status}: {json}")));
            }
            let msg = &json["choices"][0]["message"];
            let text = msg["content"].as_str().unwrap_or("").to_string();
            let tool_calls = msg["tool_calls"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| {
                            let f = &c["function"];
                            let args = f["arguments"].as_str().unwrap_or("{}");
                            Some(ToolCall {
                                id: c["id"].as_str().unwrap_or("").to_string(),
                                name: f["name"].as_str()?.to_string(),
                                arguments: serde_json::from_str(args).unwrap_or(serde_json::json!({})),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let tokens_in = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
            let tokens_out = json["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;
            Ok(Completion { text, model: req.model.clone(), tokens_in, tokens_out, tool_calls })
        }

        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, GatewayError> {
            let body = serde_json::json!({ "model": "text-embedding-3-small", "input": texts });
            let resp = self
                .client
                .post(format!("{}/embeddings", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let json: serde_json::Value =
                resp.json().await.map_err(|e| GatewayError::Provider(e.to_string()))?;
            let out = json["data"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|d| {
                            d["embedding"]
                                .as_array()
                                .map(|v| v.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(out)
        }

        fn id(&self) -> &str {
            &self.id
        }
    }
}
