//! Embeddings - turning text into a vector so meaning, not just words, can be
//! matched.
//!
//! The real semantic win over a keyword-only agent comes from a transformer
//! embedding model, which arrives through the LLM gateway. To keep the core binary
//! tiny (no bundled ONNX runtime) and to let the whole memory pipeline be built and
//! tested offline, this module ships a dependency-free default: a hashed bag of word
//! tokens and character trigrams. It is deterministic and captures morphology and
//! word-order robustness (so `prefers` matches `preferences`) that exact keyword
//! search misses - a genuine step up - while the heavier model plugs into the same
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

/// FNV-1a, a small deterministic hash - stable across platforms and versions, which
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
        for tok in lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
        {
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

/// Binary-quantize a vector to one bit per dimension, packed little-endian into bytes. This is the
/// coarse index: comparing two quantized vectors by Hamming distance approximates their cosine
/// ordering at ~1/32 the bytes and with a single XOR+popcount per byte, which is what lets recall
/// scan EVERY in-scope vector (no salience cap) and still stay fast as the brain grows.
///
/// The bit is set for dimensions ABOVE the vector's own mean, not above a fixed zero. The default
/// [`TrigramHashEmbedder`] is sparse - a short text bumps only a handful of the 256 dims, leaving
/// the rest exactly 0.0 - so a plain `x >= 0.0` test made every untouched dim quantize to 1. Codes
/// then collapsed toward all-ones and Hamming distance degenerated into "how few negative dims does
/// this row have" (i.e. text shortness), a poor discriminator that lets short unrelated rows crowd
/// out a genuine paraphrase once a ring exceeds the coarse truncation. Centering on the per-vector
/// mean makes the split track the vector's own structure instead of the origin. (The coarse pass is
/// still only a pre-filter: recall skips it entirely below a candidate threshold and reranks the
/// survivors by exact cosine - see `recall_inner`.)
pub fn quantize_binary(v: &[f32]) -> Vec<u8> {
    let nbytes = v.len().div_ceil(8);
    let mut out = vec![0u8; nbytes];
    let mean = if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    };
    for (i, &x) in v.iter().enumerate() {
        if x >= mean {
            out[i / 8] |= 1u8 << (i % 8);
        }
    }
    out
}

/// Hamming distance between two packed binary codes (lower = more similar). Bytes past the shorter
/// code count as fully differing, so a dimension/space mismatch is deprioritised rather than
/// silently ignored.
pub fn hamming(a: &[u8], b: &[u8]) -> u32 {
    let n = a.len().min(b.len());
    let mut d = 0u32;
    for i in 0..n {
        d += (a[i] ^ b[i]).count_ones();
    }
    d + (a.len().abs_diff(b.len()) as u32) * 8
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

    #[test]
    fn binary_code_hamming_tracks_cosine_order() {
        let e = TrigramHashEmbedder::default();
        let q = e.embed("the user prefers a dark theme");
        let close = e.embed("the user likes dark themes");
        let far = e.embed("the weather in Berlin is cold today");
        let (qb, cb, fb) = (
            quantize_binary(&q),
            quantize_binary(&close),
            quantize_binary(&far),
        );
        // The paraphrase is nearer in BOTH the exact cosine and the coarse Hamming ordering, so the
        // coarse pass keeps the right candidate.
        assert!(cosine(&q, &close) > cosine(&q, &far));
        assert!(
            hamming(&qb, &cb) < hamming(&qb, &fb),
            "coarse Hamming must rank the paraphrase nearer"
        );
        // The code is one bit per dimension.
        assert_eq!(qb.len(), q.len().div_ceil(8));
    }
}
