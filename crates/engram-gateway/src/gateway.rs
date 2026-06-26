//! The gateway - the single chokepoint every model and embedding call passes
//! through. One place to meter cost, enforce the taint rule, and write the audit
//! trail, so no skill or subsystem can reach a model off the record.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use engram_core::{Ledger, Taint};
use serde::Serialize;
use serde_json::json;

use crate::provider::{approx_tokens, GatewayError, Provider};
use crate::types::{Completion, CompletionRequest};

/// Price in USD per million tokens.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    pub in_per_mtok: f64,
    pub out_per_mtok: f64,
}

/// Running cost/usage counters. Cheap atomics so any thread can read a snapshot.
#[derive(Default)]
pub struct Meter {
    tokens_in: AtomicU64,
    tokens_out: AtomicU64,
    cost_micros: AtomicU64,
    calls: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeterSnapshot {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub calls: u64,
}

impl Meter {
    fn record(&self, tokens_in: u32, tokens_out: u32, cost_usd: f64) {
        self.tokens_in.fetch_add(tokens_in as u64, Ordering::Relaxed);
        self.tokens_out.fetch_add(tokens_out as u64, Ordering::Relaxed);
        self.cost_micros
            .fetch_add((cost_usd * 1_000_000.0) as u64, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot {
            tokens_in: self.tokens_in.load(Ordering::Relaxed),
            tokens_out: self.tokens_out.load(Ordering::Relaxed),
            cost_usd: self.cost_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0,
            calls: self.calls.load(Ordering::Relaxed),
        }
    }
}

/// A completion request plus its provenance: the taint it inherited and who is asking.
pub struct Call {
    pub request: CompletionRequest,
    pub taint: Taint,
    pub actor: String,
}

impl Call {
    pub fn new(request: CompletionRequest) -> Self {
        Self { request, taint: Taint::Trusted, actor: "core".into() }
    }
    pub fn tainted(mut self, taint: Taint) -> Self {
        self.taint = self.taint.join(taint);
        self
    }
    pub fn actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = actor.into();
        self
    }
}

/// The metered, taint-aware, audited gateway.
pub struct Gateway {
    /// The active model provider, behind a lock so the desktop's settings panel can
    /// swap it (a new API key, model, or backend) without restarting the daemon.
    provider: std::sync::Mutex<Arc<dyn Provider>>,
    ledger: Arc<Ledger>,
    meter: Meter,
    prices: HashMap<String, Price>,
}

impl Gateway {
    pub fn new(provider: Box<dyn Provider>, ledger: Arc<Ledger>) -> Self {
        Self {
            provider: std::sync::Mutex::new(Arc::from(provider)),
            ledger,
            meter: Meter::default(),
            prices: default_prices(),
        }
    }

    /// A cheap clone of the current provider handle. Callers hold the `Arc`, not the
    /// lock, so a long model call never blocks a settings swap (and vice versa).
    fn provider(&self) -> Arc<dyn Provider> {
        self.provider.lock().expect("provider lock").clone()
    }

    /// Hot-swap the active provider. The next call picks it up; calls already in flight
    /// finish on the old one. Used by the settings panel after the user edits the model.
    pub fn set_provider(&self, provider: Arc<dyn Provider>) {
        *self.provider.lock().expect("provider lock") = provider;
    }

    /// Identifier of the active provider (for display and audit).
    pub fn provider_id(&self) -> String {
        self.provider().id().to_string()
    }

    /// Override or add a model price (USD per million tokens).
    pub fn with_price(mut self, model: impl Into<String>, price: Price) -> Self {
        self.prices.insert(model.into(), price);
        self
    }

    pub fn meter(&self) -> MeterSnapshot {
        self.meter.snapshot()
    }

    /// Run a completion. **The taint rule**: an untrusted call (its context derived
    /// from the web or unknown memory) has every secret-bearing message stripped
    /// before it reaches the model - half of breaking the injection→exfiltration
    /// chain. The redaction is metered and written to the audit ledger.
    pub async fn complete(&self, mut call: Call) -> Result<Completion, GatewayError> {
        let mut redacted = 0usize;
        if call.taint.is_untrusted() {
            let before = call.request.messages.len();
            call.request.messages.retain(|m| !m.secret);
            redacted = before - call.request.messages.len();
            if redacted > 0 {
                tracing::warn!(redacted, actor = %call.actor, "taint: stripped secret context from untrusted call");
            }
        }

        let completion = self.provider().complete(&call.request).await?;
        self.record_call(&call, &completion, redacted)?;
        Ok(completion)
    }

    /// Like [`Gateway::complete`], but streams text fragments to `on_delta` as they arrive
    /// and returns the full completion at the end. Metering and the audit entry are written
    /// once, on completion, exactly as for a non-streaming call.
    pub async fn complete_stream(
        &self,
        mut call: Call,
        on_delta: &mut (dyn FnMut(String) + Send),
    ) -> Result<Completion, GatewayError> {
        let mut redacted = 0usize;
        if call.taint.is_untrusted() {
            let before = call.request.messages.len();
            call.request.messages.retain(|m| !m.secret);
            redacted = before - call.request.messages.len();
        }
        let completion = self.provider().complete_stream(&call.request, on_delta).await?;
        self.record_call(&call, &completion, redacted)?;
        Ok(completion)
    }

    /// Meter and audit a finished model call (shared by the streaming and non-streaming paths).
    fn record_call(
        &self,
        call: &Call,
        completion: &Completion,
        redacted: usize,
    ) -> Result<(), GatewayError> {
        let cost = self.cost(completion);
        self.meter.record(completion.tokens_in, completion.tokens_out, cost);
        self.ledger.append(
            "llm.call",
            &call.actor,
            json!({
                "provider": self.provider().id(),
                "model": completion.model,
                "tokens_in": completion.tokens_in,
                "tokens_out": completion.tokens_out,
                "cost_usd": cost,
                "taint": taint_str(call.taint),
                "redacted_secrets": redacted,
            }),
        )?;
        Ok(())
    }

    /// Embed texts through the provider, metered and audited.
    pub async fn embed(&self, texts: &[String], actor: &str) -> Result<Vec<Vec<f32>>, GatewayError> {
        let out = self.provider().embed(texts).await?;
        let tokens_in: u32 = texts.iter().map(|t| approx_tokens(t)).sum();
        self.meter.record(tokens_in, 0, 0.0);
        self.ledger.append(
            "llm.embed",
            actor,
            json!({ "provider": self.provider().id(), "count": texts.len(), "tokens_in": tokens_in }),
        )?;
        Ok(out)
    }

    /// Generate an image (PNG bytes) from a prompt, metered and audited.
    pub async fn generate_image(&self, prompt: &str, actor: &str) -> Result<Vec<u8>, GatewayError> {
        let bytes = self.provider().generate_image(prompt).await?;
        self.meter.record(approx_tokens(prompt), 0, 0.0);
        self.ledger.append(
            "llm.image",
            actor,
            json!({ "provider": self.provider().id(), "prompt_len": prompt.len(), "bytes": bytes.len() }),
        )?;
        Ok(bytes)
    }

    /// Transcribe audio bytes to text, metered and audited.
    pub async fn transcribe(&self, audio: &[u8], format: &str, actor: &str) -> Result<String, GatewayError> {
        let text = self.provider().transcribe(audio, format).await?;
        self.meter.record(0, approx_tokens(&text), 0.0);
        self.ledger.append(
            "llm.transcribe",
            actor,
            json!({ "provider": self.provider().id(), "bytes": audio.len(), "chars": text.len() }),
        )?;
        Ok(text)
    }

    /// Synthesize speech (audio bytes) from text, metered and audited.
    pub async fn tts(&self, text: &str, voice: &str, actor: &str) -> Result<Vec<u8>, GatewayError> {
        let bytes = self.provider().tts(text, voice).await?;
        self.meter.record(approx_tokens(text), 0, 0.0);
        self.ledger.append(
            "llm.tts",
            actor,
            json!({ "provider": self.provider().id(), "chars": text.len(), "bytes": bytes.len() }),
        )?;
        Ok(bytes)
    }

    fn cost(&self, c: &Completion) -> f64 {
        // Exact match, else a family match so versioned ids like "claude-haiku-4-5-..."
        // still cost against the "claude-haiku" entry.
        let price = self.prices.get(&c.model).copied().or_else(|| {
            self.prices.iter().find(|(k, _)| c.model.contains(k.as_str())).map(|(_, p)| *p)
        });
        match price {
            Some(p) => {
                (c.tokens_in as f64 / 1_000_000.0) * p.in_per_mtok
                    + (c.tokens_out as f64 / 1_000_000.0) * p.out_per_mtok
            }
            None => 0.0,
        }
    }
}

fn taint_str(t: Taint) -> &'static str {
    if t.is_untrusted() {
        "untrusted"
    } else {
        "trusted"
    }
}

/// Approximate public list prices (USD / 1M tokens), used for cost metering. These
/// are illustrative defaults; override with [`Gateway::with_price`].
fn default_prices() -> HashMap<String, Price> {
    let mut m = HashMap::new();
    m.insert("claude-haiku".into(), Price { in_per_mtok: 1.0, out_per_mtok: 5.0 });
    m.insert("claude-sonnet".into(), Price { in_per_mtok: 3.0, out_per_mtok: 15.0 });
    m.insert("claude-opus".into(), Price { in_per_mtok: 15.0, out_per_mtok: 75.0 });
    m.insert("gpt-4o-mini".into(), Price { in_per_mtok: 0.15, out_per_mtok: 0.60 });
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;
    use crate::types::{CompletionRequest, Message};

    fn gw() -> (Gateway, Arc<Ledger>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let gw = Gateway::new(Box::new(MockProvider), ledger.clone());
        (gw, ledger, dir)
    }

    fn last_payload(ledger: &Ledger) -> serde_json::Value {
        let last = ledger.read_all().unwrap().pop().unwrap();
        serde_json::from_str(last.payload.get()).unwrap()
    }

    #[tokio::test]
    async fn meters_and_audits_a_call() {
        let (gw, ledger, _d) = gw();
        let req = CompletionRequest::new("claude-haiku", vec![Message::user("hello there")]);
        let c = gw.complete(Call::new(req)).await.unwrap();
        assert!(c.text.contains("mock"));
        let m = gw.meter();
        assert_eq!(m.calls, 1);
        assert!(m.tokens_in > 0 && m.tokens_out > 0);
        assert_eq!(ledger.read_all().unwrap().pop().unwrap().kind, "llm.call");
        assert_eq!(last_payload(&ledger)["redacted_secrets"], 0);
    }

    #[tokio::test]
    async fn taint_strips_secret_context() {
        let (gw, ledger, _d) = gw();
        let req = CompletionRequest::new(
            "claude-haiku",
            vec![
                Message::system("API_KEY=super-secret-value").secret(),
                Message::user("summarize this web page"),
            ],
        );
        // An untrusted call must drop the secret system message.
        gw.complete(Call::new(req).tainted(Taint::Untrusted)).await.unwrap();
        let p = last_payload(&ledger);
        assert_eq!(p["redacted_secrets"], 1);
        assert_eq!(p["taint"], "untrusted");
    }

    #[tokio::test]
    async fn trusted_call_keeps_secret() {
        let (gw, ledger, _d) = gw();
        let req = CompletionRequest::new(
            "claude-haiku",
            vec![Message::system("private").secret(), Message::user("hi")],
        );
        gw.complete(Call::new(req)).await.unwrap();
        assert_eq!(last_payload(&ledger)["redacted_secrets"], 0);
    }

    #[tokio::test]
    async fn cost_accumulates_for_known_model() {
        let (gw, _l, _d) = gw();
        for _ in 0..3 {
            let req = CompletionRequest::new("claude-opus", vec![Message::user("count the cost")]);
            gw.complete(Call::new(req)).await.unwrap();
        }
        assert!(gw.meter().cost_usd > 0.0);
        assert_eq!(gw.meter().calls, 3);
    }
}
