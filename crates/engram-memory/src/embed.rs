//! Embeddings — turning text into a vector so meaning, not just words, can be
//! matched.
//!
//! The real semantic win over a keyword-only agent comes from a transformer
//! embedding model, which arrives through the LLM gateway. To keep the core binary
//! tiny (no bundled ONNX runtime) and to let the whole memory pipeline be built and
//! tested offline, this module ships a dependency-free default: a hashed bag of word
//! tokens and character trigrams. It is deterministic and captures morphology and
//! word-order robustness (so `prefers` matches `preferences`) that exact keyword
//! search misses — a genuine step up — while the heavier model plugs into the same
//! [`Embedder`] trait when present.

/// Anything that can turn text into a fixed-width vector.
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, text: &str) -> Vec<f32>;
    fn name(&self) -> &str;
}

/// Cosine similarity. Inputs need not be pre-normalised.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// FNV-1a, a small deterministic hash — stable across platforms and versions, which
/// matters because embeddings are persisted.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The offline default embedder: signed feature hashing over word tokens and padded
/// character trigrams, L2-normalised.
pub struct TrigramHashEmbedder {
    dim: usize,
}

impl TrigramHashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(8) }
    }
}

impl Default for TrigramHashEmbedder {
    fn default() -> Self {
        Self::new(256)
    }
}

impl Embedder for TrigramHashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &str {
        "trigram-hash-v1"
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        let lower = text.to_lowercase();
        for tok in lower.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
            bump(&mut v, format!("w:{tok}").as_bytes());
            let padded = format!("#{tok}#");
            let chars: Vec<char> = padded.chars().collect();
            for w in chars.windows(3) {
                let tri: String = w.iter().collect();
                bump(&mut v, format!("t:{tri}").as_bytes());
            }
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
        v
    }
}

fn bump(v: &mut [f32], key: &[u8]) {
    let n = v.len();
    let h = fnv1a(key);
    let idx = (h as usize) % n;
    let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
    v[idx] += sign;
}

/// Pack a vector into little-endian bytes for BLOB storage.
pub fn to_bytes(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

/// Unpack a vector from little-endian bytes.
pub fn from_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morphology_beats_exact_match() {
        let e = TrigramHashEmbedder::default();
        let prefs = e.embed("the user preferences and theme");
        let q = e.embed("preferred theming");
        let unrelated = e.embed("the weather in Berlin is cold");
        // Morphological overlap (prefer*, theme/theming) scores higher than an
        // unrelated sentence sharing only stopwords.
        assert!(cosine(&q, &prefs) > cosine(&q, &unrelated));
    }

    #[test]
    fn roundtrips_bytes() {
        let e = TrigramHashEmbedder::new(32);
        let v = e.embed("hello world");
        assert_eq!(from_bytes(&to_bytes(&v)), v);
    }
}
