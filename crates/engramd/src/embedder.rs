//! The bridge that lets memory use a real embedding model.
//!
//! [`engram_memory::Embedder`] is synchronous (memory writes and recalls call it
//! inline), while the gateway is async. `GatewayEmbedder` bridges them with
//! `block_in_place` + `block_on`, so on the daemon's multi-thread runtime a transformer
//! embedding model — reached through the same metered, audited gateway as everything
//! else — slots in behind the existing trait. Selected with `ENGRAM_EMBED=gateway`;
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
}

impl GatewayEmbedder {
    /// Construct from a gateway and the embedding dimension (probe it once at startup).
    /// Must be called from within a Tokio runtime (captures the current handle).
    pub fn new(gateway: Arc<Gateway>, dim: usize) -> Self {
        Self { gateway, handle: Handle::current(), dim: dim.max(1) }
    }
}

impl Embedder for GatewayEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &str {
        "gateway"
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let texts = vec![text.to_string()];
        // block_in_place hands this worker back to the runtime so block_on is legal on
        // the multi-thread runtime the daemon uses.
        let result =
            tokio::task::block_in_place(|| self.handle.block_on(self.gateway.embed(&texts, "memory")));
        match result {
            Ok(mut v) if !v.is_empty() => v.swap_remove(0),
            _ => vec![0.0; self.dim],
        }
    }
}
