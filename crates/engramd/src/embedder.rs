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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use engram_gateway::Gateway;
use engram_memory::{Embedder, StaticEmbedder};
use tokio::runtime::Handle;

/// The pinned model2vec export the one-click "upgrade to a real semantic embedder" fetch
/// downloads - the SAME model verified end-to-end in `crates/engram-bench/BENCHMARKS.md` §1
/// (100% recall@10 on the labeled benchmark corpus, matching mem0/LangChain's own real embedding
/// models - see BENCHMARKS.md §3). Deliberately a single hardcoded, well-known HTTPS source, not
/// a user-suppliable URL: this is one known-good model, not a general fetch tool.
const STATIC_MODEL_TOKENIZER_URL: &str =
    "https://huggingface.co/minishlab/potion-base-8M/resolve/main/tokenizer.json";
const STATIC_MODEL_WEIGHTS_URL: &str =
    "https://huggingface.co/minishlab/potion-base-8M/resolve/main/model.safetensors";

/// Download the pinned static (model2vec) embedding model into `<home>/models/static-v1/` - the
/// one-click path from Engram's zero-dependency trigram-hash default to the real semantic
/// embedder BENCHMARKS.md shows matches mem0/LangChain on raw recall. **User-initiated only,
/// never called automatically** - offline-by-default means Engram never reaches for the network
/// unless asked (see `/v1/embedder/fetch-model`, `engram model fetch`).
///
/// A failed, partial, or corrupt download can never become "the active model": the files land in
/// a temp directory first and are proven to actually load as a real `StaticEmbedder` (see
/// [`commit_downloaded_model`]) before the atomic rename that makes them live.
pub async fn fetch_static_model(home: &Path) -> Result<PathBuf, String> {
    let tmp_dir = home.join("models").join(".static-v1.download");
    let _ = std::fs::remove_dir_all(&tmp_dir); // clean up any half-finished prior attempt
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("create temp dir: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    for (url, filename) in [
        (STATIC_MODEL_TOKENIZER_URL, "tokenizer.json"),
        (STATIC_MODEL_WEIGHTS_URL, "model.safetensors"),
    ] {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("GET {url}: HTTP {}", resp.status()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("read body {url}: {e}"))?;
        std::fs::write(tmp_dir.join(filename), &bytes)
            .map_err(|e| format!("write {filename}: {e}"))?;
    }

    let target_dir = home.join("models").join("static-v1");
    commit_downloaded_model(&tmp_dir, &target_dir)
}

/// The offline-testable half of [`fetch_static_model`]: validate that `tmp_dir` actually contains
/// a loadable model2vec export, then atomically replace `target_dir` with it. Rejects (and leaves
/// `target_dir` untouched) on any load failure — a corrupt or incomplete download must never
/// silently become "the active model" a real daemon boots into.
fn commit_downloaded_model(tmp_dir: &Path, target_dir: &Path) -> Result<PathBuf, String> {
    StaticEmbedder::load(tmp_dir).map_err(|e| format!("downloaded model failed to load: {e}"))?;
    let _ = std::fs::remove_dir_all(target_dir);
    std::fs::create_dir_all(target_dir.parent().ok_or("target_dir has no parent")?)
        .map_err(|e| format!("create models dir: {e}"))?;
    std::fs::rename(tmp_dir, target_dir).map_err(|e| format!("finalize model dir: {e}"))?;
    Ok(target_dir.to_path_buf())
}

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
        self.embed_checked(text).0
    }

    fn embed_checked(&self, text: &str) -> (Vec<f32>, bool) {
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
                Ok(mut v) if !v.is_empty() && v[0].len() == self.dim => {
                    return (v.swap_remove(0), false)
                }
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
        // provider-healthy re-embed pass runs. The `true` return lets the caller (Memory::remember)
        // mark the row `needs_reembed` so a background pass can find and repair it once the
        // provider is healthy again, closing the gap this used to just warn about and forget.
        tracing::warn!(
            embedder = %self.name,
            "gateway embedding failed after {MAX_ATTEMPTS} attempts — using a same-dimension \
             fallback vector; this record will be mis-ranked until re-embedded"
        );
        (self.fallback.embed(text), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::Ledger;
    use engram_gateway::{Gateway, MockProvider};

    // block_in_place (used by GatewayEmbedder::embed_checked) requires the multi-thread runtime.
    #[tokio::test(flavor = "multi_thread")]
    async fn healthy_call_reports_not_degraded() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let gateway = Arc::new(Gateway::new(Box::new(MockProvider), ledger));
        // MockProvider's embeddings are 8-dim; matching that dimension is the "healthy" path.
        let e = GatewayEmbedder::new(gateway, 8, "mock-model");
        let (v, degraded) = e.embed_checked("hello world");
        assert!(
            !degraded,
            "a dimension-matching mock reply must not be reported as degraded"
        );
        assert_eq!(v.len(), 8);
    }

    // block_in_place (used by GatewayEmbedder::embed_checked) requires the multi-thread runtime.
    #[tokio::test(flavor = "multi_thread")]
    async fn dimension_mismatch_falls_back_and_reports_degraded() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path()).unwrap());
        let gateway = Arc::new(Gateway::new(Box::new(MockProvider), ledger));
        // Ask for 256 dims from a provider that always replies with 8 - the exact failure mode a
        // misconfigured/incompatible embeddings endpoint produces. This must fall back to a
        // same-dimension trigram vector AND report `needs_reembed`-worthy degradation, closing the
        // gap the embedder.rs NEEDS-INTEGRATION comment used to just warn about and forget.
        let e = GatewayEmbedder::new(gateway, 256, "mock-model");
        let (v, degraded) = e.embed_checked("hello world");
        assert!(
            degraded,
            "a wrong-dimension reply must be reported as degraded"
        );
        assert_eq!(
            v.len(),
            256,
            "the fallback vector must still match the configured dimension"
        );
        // embed() (the plain trait method every other call site uses) must expose the same fallback
        // behavior, just without the degraded flag - it should never panic or return a mismatched
        // dimension either.
        assert_eq!(e.embed("hello again").len(), 256);
    }

    fn write_tiny_model(dir: &std::path::Path) {
        let tok = serde_json::json!({
            "normalizer": { "type": "BertNormalizer", "lowercase": true },
            "model": { "type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##",
                "vocab": { "[UNK]": 0, "rust": 1, "car": 2, "##s": 3 } }
        });
        std::fs::write(dir.join("tokenizer.json"), tok.to_string()).unwrap();
        let vals: Vec<f32> = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.6, 0.8];
        let data: Vec<u8> = vals.iter().flat_map(|x| x.to_le_bytes()).collect();
        let hdr = serde_json::json!({ "embeddings": { "dtype": "F32", "shape": [4, 2], "data_offsets": [0, data.len()] } });
        let hbytes = hdr.to_string().into_bytes();
        let mut st = Vec::new();
        st.extend_from_slice(&(hbytes.len() as u64).to_le_bytes());
        st.extend_from_slice(&hbytes);
        st.extend_from_slice(&data);
        std::fs::write(dir.join("model.safetensors"), st).unwrap();
    }

    #[test]
    fn commit_downloaded_model_replaces_the_target_on_a_valid_download() {
        let home = tempfile::tempdir().unwrap();
        let tmp = home.path().join("models").join(".static-v1.download");
        std::fs::create_dir_all(&tmp).unwrap();
        write_tiny_model(&tmp);
        let target = home.path().join("models").join("static-v1");

        let out = commit_downloaded_model(&tmp, &target).unwrap();
        assert_eq!(out, target);
        assert!(target.join("tokenizer.json").exists());
        assert!(target.join("model.safetensors").exists());
        assert!(
            !tmp.exists(),
            "the temp download dir must be consumed by the rename"
        );
        // The committed model must actually load and embed - proof the commit isn't just a file
        // copy, the thing it copied is real.
        let e = StaticEmbedder::load(&target).unwrap();
        assert_eq!(e.embed("rust").len(), 2);
    }

    #[test]
    fn a_corrupt_download_never_becomes_the_active_model() {
        let home = tempfile::tempdir().unwrap();
        let tmp = home.path().join("models").join(".static-v1.download");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("tokenizer.json"), b"not valid json at all").unwrap();
        std::fs::write(tmp.join("model.safetensors"), b"garbage").unwrap();
        let target = home.path().join("models").join("static-v1");
        // Pre-existing good model must survive an aborted/corrupt fetch attempt.
        std::fs::create_dir_all(&target).unwrap();
        write_tiny_model(&target);

        let err = commit_downloaded_model(&tmp, &target);
        assert!(
            err.is_err(),
            "a corrupt download must be rejected, not committed"
        );
        assert!(
            target.join("tokenizer.json").exists(),
            "the previously-good model must be left untouched on a failed commit"
        );
        StaticEmbedder::load(&target).expect("the untouched prior model must still load");
    }

    // Real network test (hits huggingface.co) - not part of the default offline suite, matching
    // the same pattern as `WebFetchTool`'s `fetches_a_real_page` test. Run explicitly:
    //   cargo test -p engramd --release -- --ignored fetch_static_model_downloads_a_real_model
    #[tokio::test]
    #[ignore]
    async fn fetch_static_model_downloads_a_real_model() {
        let home = tempfile::tempdir().unwrap();
        let dir = fetch_static_model(home.path()).await.unwrap();
        let e = StaticEmbedder::load(&dir).unwrap();
        assert!(e.dim() > 0);
    }
}
