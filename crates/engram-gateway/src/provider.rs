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
    /// Stream a completion, calling `on_delta` with each text fragment as it arrives, and
    /// returning the full completion at the end. The default is a non-streaming fallback
    /// (produce the whole completion, emit its text once) so every provider works; real
    /// streaming providers override it.
    async fn complete_stream(
        &self,
        req: &CompletionRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
    ) -> Result<Completion, GatewayError> {
        let c = self.complete(req).await?;
        if !c.text.is_empty() {
            on_delta(c.text.clone());
        }
        Ok(c)
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, GatewayError>;
    /// Generate an image from a prompt, returning PNG bytes. Default: unsupported.
    async fn generate_image(&self, _prompt: &str) -> Result<Vec<u8>, GatewayError> {
        Err(GatewayError::Provider("image generation not supported by this provider".into()))
    }
    /// Synthesize speech from text, returning audio bytes (mp3). Default: unsupported.
    async fn tts(&self, _text: &str, _voice: &str) -> Result<Vec<u8>, GatewayError> {
        Err(GatewayError::Provider("text-to-speech not supported by this provider".into()))
    }
    /// Transcribe audio bytes (of the given format, e.g. "mp3"/"wav") to text. Default: unsupported.
    async fn transcribe(&self, _audio: &[u8], _format: &str) -> Result<String, GatewayError> {
        Err(GatewayError::Provider("speech-to-text not supported by this provider".into()))
    }
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
pub use anthropic::AnthropicProvider;
#[cfg(feature = "http")]
pub use http::HttpProvider;

#[cfg(feature = "http")]
mod anthropic {
    //! A native Anthropic Messages-API provider. Unlike the OpenAI-compatible path, this
    //! puts the system prompt in its own field, uses `tool_use`/`tool_result` content
    //! blocks, authenticates with `x-api-key`, and — the reason it exists — marks the
    //! stable tools+system prefix with `cache_control`, so Anthropic prompt-caches it
    //! across the agent loop's many turns (large cost/latency win on long runs).

    use super::*;
    use crate::types::ToolCall;
    use futures_util::StreamExt;

    pub struct AnthropicProvider {
        client: reqwest::Client,
        base_url: String,
        api_key: String,
    }

    impl AnthropicProvider {
        pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
            let base = base_url.into();
            let base = if base.trim().is_empty() {
                "https://api.anthropic.com/v1".to_string()
            } else {
                base.trim_end_matches('/').to_string()
            };
            Self { client: reqwest::Client::new(), base_url: base, api_key: api_key.into() }
        }
    }

    /// Append `block` to the last message if it has the same role, else start a new one —
    /// so consecutive tool results (from parallel tool calls) collapse into one user
    /// message with several `tool_result` blocks, keeping Anthropic's role alternation.
    fn push_block(msgs: &mut Vec<serde_json::Value>, role: &str, block: serde_json::Value) {
        if let Some(last) = msgs.last_mut() {
            if last["role"] == role {
                if let Some(arr) = last["content"].as_array_mut() {
                    arr.push(block);
                    return;
                }
            }
        }
        msgs.push(serde_json::json!({ "role": role, "content": [block] }));
    }

    /// Convert a provider-agnostic request into an Anthropic Messages body. Pure and
    /// unit-tested: system is lifted out and marked cacheable; assistant tool calls become
    /// `tool_use` blocks; tool results become `tool_result` blocks on a user message.
    pub(crate) fn anthropic_body(req: &CompletionRequest) -> serde_json::Value {
        let mut system_text = String::new();
        let mut msgs: Vec<serde_json::Value> = Vec::new();
        for m in &req.messages {
            match m.role {
                Role::System => {
                    if !system_text.is_empty() {
                        system_text.push_str("\n\n");
                    }
                    system_text.push_str(&m.content);
                }
                Role::User => {
                    push_block(&mut msgs, "user", serde_json::json!({ "type": "text", "text": m.content }));
                    for img in &m.images {
                        push_block(
                            &mut msgs,
                            "user",
                            serde_json::json!({
                                "type": "image",
                                "source": { "type": "base64", "media_type": "image/png", "data": img }
                            }),
                        );
                    }
                }
                Role::Assistant => {
                    if !m.content.is_empty() {
                        push_block(&mut msgs, "assistant", serde_json::json!({ "type": "text", "text": m.content }));
                    }
                    for tc in &m.tool_calls {
                        push_block(
                            &mut msgs,
                            "assistant",
                            serde_json::json!({ "type": "tool_use", "id": tc.id, "name": tc.name, "input": tc.arguments }),
                        );
                    }
                }
                Role::Tool => {
                    push_block(
                        &mut msgs,
                        "user",
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                            "content": m.content
                        }),
                    );
                }
            }
        }
        // A cache breakpoint at the end of the conversation: Anthropic caches the whole
        // prefix up to here (tools + system + every prior turn) and, on the next turn, reads
        // the longest matching cached prefix — so the big, growing agent-loop context is
        // re-read at ~0.1x instead of reprocessed each step.
        if let Some(last) = msgs.last_mut() {
            if let Some(blocks) = last["content"].as_array_mut() {
                if let Some(block) = blocks.last_mut() {
                    block["cache_control"] = serde_json::json!({ "type": "ephemeral" });
                }
            }
        }
        let mut body = serde_json::json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "messages": msgs,
        });
        if !system_text.is_empty() {
            // cache_control on the system block caches the tools+system prefix (Anthropic's
            // cache order is tools -> system -> messages), reused across the loop's turns.
            body["system"] = serde_json::json!([
                { "type": "text", "text": system_text, "cache_control": { "type": "ephemeral" } }
            ]);
        }
        if !req.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(
                req.tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({ "name": t.name, "description": t.description, "input_schema": t.parameters })
                    })
                    .collect(),
            );
        }
        body
    }

    /// Pull complete SSE events (blank-line separated) out of `buf`, leaving any partial
    /// trailing event for the next chunk. Calls `on_data` with each parsed `data:` JSON.
    fn drain_sse(buf: &mut String, mut on_data: impl FnMut(&serde_json::Value)) {
        while let Some(pos) = buf.find("\n\n") {
            let frame: String = buf.drain(..pos + 2).collect();
            for line in frame.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                        on_data(&v);
                    }
                }
            }
        }
    }

    #[async_trait]
    impl Provider for AnthropicProvider {
        async fn complete(&self, req: &CompletionRequest) -> Result<Completion, GatewayError> {
            let body = anthropic_body(req);
            let resp = self
                .client
                .post(format!("{}/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
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
            let mut text = String::new();
            let mut tool_calls = Vec::new();
            if let Some(blocks) = json["content"].as_array() {
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => text.push_str(b["text"].as_str().unwrap_or("")),
                        Some("tool_use") => tool_calls.push(ToolCall {
                            id: b["id"].as_str().unwrap_or("").to_string(),
                            name: b["name"].as_str().unwrap_or("").to_string(),
                            arguments: b["input"].clone(),
                        }),
                        _ => {}
                    }
                }
            }
            let u = &json["usage"];
            let cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let cache_create = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            tracing::debug!(
                input = u["input_tokens"].as_u64().unwrap_or(0),
                cache_create,
                cache_read,
                "anthropic usage"
            );
            // Count all processed input (fresh + cache create + cache read) for metering;
            // cost stays a conservative upper bound (no cache discount applied).
            let tokens_in =
                (u["input_tokens"].as_u64().unwrap_or(0) + cache_read + cache_create) as u32;
            let tokens_out = u["output_tokens"].as_u64().unwrap_or(0) as u32;
            Ok(Completion { text, model: req.model.clone(), tokens_in, tokens_out, tool_calls })
        }

        async fn complete_stream(
            &self,
            req: &CompletionRequest,
            on_delta: &mut (dyn FnMut(String) + Send),
        ) -> Result<Completion, GatewayError> {
            let mut body = anthropic_body(req);
            body["stream"] = serde_json::json!(true);
            let resp = self
                .client
                .post(format!("{}/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(GatewayError::Provider(format!("{status}: {txt}")));
            }
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            let mut text = String::new();
            let (mut tin, mut tout, mut cread, mut ccreate) = (0u64, 0u64, 0u64, 0u64);
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.map_err(|e| GatewayError::Provider(e.to_string()))?;
                buf.push_str(&String::from_utf8_lossy(&bytes));
                drain_sse(&mut buf, |v| match v["type"].as_str() {
                    Some("content_block_delta") if v["delta"]["type"] == "text_delta" => {
                        if let Some(t) = v["delta"]["text"].as_str() {
                            text.push_str(t);
                            on_delta(t.to_string());
                        }
                    }
                    Some("message_start") => {
                        let u = &v["message"]["usage"];
                        tin += u["input_tokens"].as_u64().unwrap_or(0);
                        cread += u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                        ccreate += u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                    }
                    Some("message_delta") => {
                        tout += v["usage"]["output_tokens"].as_u64().unwrap_or(0);
                    }
                    _ => {}
                });
            }
            Ok(Completion {
                text,
                model: req.model.clone(),
                tokens_in: (tin + cread + ccreate) as u32,
                tokens_out: tout as u32,
                tool_calls: Vec::new(),
            })
        }

        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, GatewayError> {
            Err(GatewayError::Provider(
                "anthropic has no embeddings endpoint — use the offline embedder or a separate provider"
                    .into(),
            ))
        }

        fn id(&self) -> &str {
            "anthropic"
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::types::{Message, ToolCall, ToolDef};

        #[test]
        fn lifts_system_marks_it_cacheable_and_shapes_tools_and_blocks() {
            let req = CompletionRequest::new(
                "claude-haiku-4-5",
                vec![
                    Message::system("you are engram"),
                    Message::user("hi"),
                    Message::assistant_tool_calls(
                        "",
                        vec![ToolCall { id: "t1".into(), name: "echo".into(), arguments: serde_json::json!({ "x": 1 }) }],
                    ),
                    Message::tool_result("t1", "echoed"),
                ],
            )
            .tools(vec![ToolDef {
                name: "echo".into(),
                description: "e".into(),
                parameters: serde_json::json!({ "type": "object" }),
            }]);
            let body = anthropic_body(&req);

            // System is lifted out of messages into a cacheable block.
            assert_eq!(body["system"][0]["text"], "you are engram");
            assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
            // Tools use Anthropic's input_schema shape.
            assert_eq!(body["tools"][0]["name"], "echo");
            assert!(body["tools"][0]["input_schema"].is_object());
            // Messages: user, assistant(tool_use), user(tool_result).
            let msgs = body["messages"].as_array().unwrap();
            assert_eq!(msgs[0]["role"], "user");
            assert_eq!(msgs[1]["role"], "assistant");
            assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
            assert_eq!(msgs[1]["content"][0]["id"], "t1");
            assert_eq!(msgs[2]["role"], "user");
            assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
            assert_eq!(msgs[2]["content"][0]["tool_use_id"], "t1");
            // A second cache breakpoint sits at the end of the conversation so the whole
            // growing prefix is cached and re-read on the next turn (incremental caching).
            let last = msgs.last().unwrap()["content"].as_array().unwrap().last().unwrap();
            assert_eq!(last["cache_control"]["type"], "ephemeral");
        }

        #[test]
        fn merges_parallel_tool_results_into_one_user_message() {
            let req = CompletionRequest::new(
                "m",
                vec![
                    Message::user("go"),
                    Message::assistant_tool_calls(
                        "",
                        vec![
                            ToolCall { id: "a".into(), name: "x".into(), arguments: serde_json::json!({}) },
                            ToolCall { id: "b".into(), name: "y".into(), arguments: serde_json::json!({}) },
                        ],
                    ),
                    Message::tool_result("a", "ra"),
                    Message::tool_result("b", "rb"),
                ],
            );
            let body = anthropic_body(&req);
            let msgs = body["messages"].as_array().unwrap();
            assert_eq!(msgs.len(), 3); // user, assistant(2 tool_use), user(2 tool_result)
            assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);
            assert_eq!(msgs[2]["content"][0]["tool_use_id"], "a");
            assert_eq!(msgs[2]["content"][1]["tool_use_id"], "b");
        }

        #[test]
        fn drain_sse_extracts_text_deltas_and_buffers_a_partial_event() {
            let mut buf = String::new();
            buf.push_str("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n");
            buf.push_str("data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n");
            buf.push_str("data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n");
            // A partial event (no terminating blank line yet) must NOT be consumed.
            buf.push_str("data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"!\"}}");

            let mut got = String::new();
            let collect = |out: &mut String, v: &serde_json::Value| {
                if v["delta"]["type"] == "text_delta" {
                    out.push_str(v["delta"]["text"].as_str().unwrap_or(""));
                }
            };
            drain_sse(&mut buf, |v| collect(&mut got, v));
            assert_eq!(got, "Hello world"); // the partial "!" is still buffered
            assert!(buf.contains("\"!\""));

            buf.push_str("\n\n"); // the rest of the chunk arrives
            drain_sse(&mut buf, |v| collect(&mut got, v));
            assert_eq!(got, "Hello world!");
            assert!(buf.is_empty());
        }
    }
}

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
        if !m.images.is_empty() {
            // Multimodal content: text part plus image_url parts (data URLs).
            let mut parts = vec![serde_json::json!({ "type": "text", "text": m.content })];
            for img in &m.images {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": format!("data:image/png;base64,{img}") }
                }));
            }
            o["content"] = serde_json::Value::Array(parts);
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

        async fn generate_image(&self, prompt: &str) -> Result<Vec<u8>, GatewayError> {
            use base64::Engine;
            let model = std::env::var("ENGRAM_IMAGE_MODEL").unwrap_or_else(|_| "gpt-image-1".into());
            let body = serde_json::json!({
                "model": model, "prompt": prompt, "n": 1, "size": "1024x1024", "response_format": "b64_json"
            });
            let resp = self
                .client
                .post(format!("{}/images/generations", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let json: serde_json::Value =
                resp.json().await.map_err(|e| GatewayError::Provider(e.to_string()))?;
            let b64 = json["data"][0]["b64_json"]
                .as_str()
                .ok_or_else(|| GatewayError::Provider(format!("no image in response: {json}")))?;
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| GatewayError::Provider(e.to_string()))
        }

        async fn tts(&self, text: &str, voice: &str) -> Result<Vec<u8>, GatewayError> {
            let model = std::env::var("ENGRAM_TTS_MODEL").unwrap_or_else(|_| "tts-1".into());
            let body = serde_json::json!({ "model": model, "input": text, "voice": voice });
            let resp = self
                .client
                .post(format!("{}/audio/speech", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let bytes = resp.bytes().await.map_err(|e| GatewayError::Provider(e.to_string()))?;
            Ok(bytes.to_vec())
        }

        async fn transcribe(&self, audio: &[u8], format: &str) -> Result<String, GatewayError> {
            let model = std::env::var("ENGRAM_STT_MODEL").unwrap_or_else(|_| "whisper-1".into());
            let mime = match format {
                "mp3" => "audio/mpeg",
                "wav" => "audio/wav",
                "m4a" | "mp4" => "audio/mp4",
                "ogg" => "audio/ogg",
                _ => "application/octet-stream",
            };
            let part = reqwest::multipart::Part::bytes(audio.to_vec())
                .file_name(format!("audio.{format}"))
                .mime_str(mime)
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let form = reqwest::multipart::Form::new()
                .part("file", part)
                .text("model", model)
                .text("response_format", "json");
            let resp = self
                .client
                .post(format!("{}/audio/transcriptions", self.base_url))
                .bearer_auth(&self.api_key)
                .multipart(form)
                .send()
                .await
                .map_err(|e| GatewayError::Provider(e.to_string()))?;
            let json: serde_json::Value =
                resp.json().await.map_err(|e| GatewayError::Provider(e.to_string()))?;
            Ok(json["text"].as_str().unwrap_or("").to_string())
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
