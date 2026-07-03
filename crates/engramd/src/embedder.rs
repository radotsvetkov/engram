//! The bridge that lets memory use a real embedding model.
//!
//! [`engram_memory::Embedder`] is synchronous (memory writes and recalls call it
//! inline), while the gateway is async. `GatewayEmbedder` bridges them with
//! `block_in_place` + `block_on`, so on the daemon's multi-thread runtime a transformer
//! embedding model - reached through the same metered, audited gateway as everything
//! else - slots in behind the existing trait. Selected with `ENGRAM_EMBED=gateway`;
//! the offline trigram embedder remains the default.
//!
//! Note: the embedding dimension must match across a brain's lifetime, so switching
//! embedders means starting with a fresh `ENGRAM_HOME`.

use std::sync::Arc;

use engram_gateway::Gateway;
use engram_memory::Embedder;
use tokio::runtime::Handle;

pub struct GatewayEmbedder {
    gateway: Arc<Gateway>,
    handle: Handle,
    dim: usize,
    /// The embed-space identity, including the provider + model. Memory keys stored vectors by
    /// this name; making it model-specific means switching to a DIFFERENT same-dimension model
    /// forces a re-embed instead of silently comparing vectors from incompatible spaces.
    name: String,
    /// A dependency-free fallback used (at the SAME dimension) only when the provider's embedding
    /// call fails, so a transient outage degrades gracefully instead of persisting a zero vector
    /// that would make that memory permanently unfindable.
    fallback: engram_memory::TrigramHashEmbedder,
}

impl GatewayEmbedder {
    /// Construct from a gateway and the embedding dimension (probe it once at startup).
    /// Must be called from within a Tokio runtime (captures the current handle).
    pub fn new(gateway: Arc<Gateway>, dim: usize, model: &str) -> Self {
        let name = format!("gateway:{}:{}", gateway.provider_id(), model);
        let dim = dim.max(1);
        Self {
            gateway,
            handle: Handle::current(),
            dim,
            name,
            fallback: engram_memory::TrigramHashEmbedder::new(dim),
        }
    }
}

impl Embedder for GatewayEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let texts = vec![text.to_string()];
        // block_in_place hands this worker back to the runtime so block_on is legal on
        // the multi-thread runtime the daemon uses. Retry a few times with a short backoff first:
        // the common failure is a transient blip (a 429 or a brief outage), and a fallback vector
        // lives in a DIFFERENT embedding space than the model's — cosine similarity between the two
        // is meaningless, so any memory embedded via fallback is mis-ranked/unfindable even after the
        // provider recovers, with no marker to re-embed it. Retrying rides out the blip so we keep the
        // whole brain in one comparable space; the fallback is the last resort, not the first reflex.
        const MAX_ATTEMPTS: usize = 3;
        for attempt in 0..MAX_ATTEMPTS {
            let result = tokio::task::block_in_place(|| {
                self.handle.block_on(self.gateway.embed(&texts, "memory"))
            });
            match result {
                Ok(mut v) if !v.is_empty() && v[0].len() == self.dim => return v.swap_remove(0),
                // A wrong-dimension reply is a configuration error, not a transient one — retrying
                // can't fix it, so don't spin; fall through to the fallback immediately.
                Ok(_) => break,
                Err(_) if attempt + 1 < MAX_ATTEMPTS => {
                    // Short, bounded backoff (25ms, 50ms). embed() is called inline on writes, so we
                    // keep the total stall small rather than hammering or hanging the write path.
                    let backoff = std::time::Duration::from_millis(25u64 << attempt);
                    tokio::task::block_in_place(|| {
                        self.handle.block_on(tokio::time::sleep(backoff))
                    });
                }
                Err(_) => break,
            }
        }
        // Provider still unavailable after retries (or a wrong-dim reply): fall back to a
        // same-dimension trigram vector rather than persisting zeros (which would silently drop the
        // memory from recall entirely). This degrades recall QUALITY for these records until a
        // provider-healthy re-embed pass runs — see NEEDS-INTEGRATION: fallback-embedded records
        // should be tagged so a background pass can re-embed them in the model's space.
        tracing::warn!(
            embedder = %self.name,
            "gateway embedding failed after {MAX_ATTEMPTS} attempts — using a same-dimension \
             fallback vector; this record will be mis-ranked until re-embedded"
        );
        self.fallback.embed(text)
    }
}
