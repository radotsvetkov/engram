//! Static (model2vec) embeddings — real synonym/paraphrase capture, in pure Rust.
//!
//! A model2vec model is a distilled static embedding table: a `[vocab, dim]` matrix where
//! every token already carries the PCA-reduced, zipf-weighted meaning learned by a real
//! sentence transformer. There is **no neural network at inference** — embedding a string
//! is just: tokenize (BERT WordPiece) → look up each token's row → mean → L2-normalize. So
//! this needs no ONNX runtime and no heavy ML crate: a hand-rolled WordPiece tokenizer, the
//! vocab from `tokenizer.json`, and the matrix read straight out of `model.safetensors`. The
//! binary stays tiny; the model is a data directory fetched/built separately at deploy time.
//!
//! Build a model directory with `scripts/build_embedder.py` (one-time, uses model2vec) or
//! point `ENGRAM_STATIC_MODEL` at any model2vec export (`tokenizer.json` + `model.safetensors`).

use std::collections::HashMap;
use std::path::Path;

use crate::embed::Embedder;

/// A loaded model2vec static embedder.
pub struct StaticEmbedder {
    vocab: HashMap<String, u32>,
    matrix: Vec<f32>,
    rows: usize,
    dim: usize,
    unk: u32,
    lowercase: bool,
}

impl StaticEmbedder {
    /// Load a model2vec model directory: `tokenizer.json` (BERT WordPiece vocab + casing)
    /// and `model.safetensors` (the `embeddings` `[vocab, dim]` f32 matrix).
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();

        // --- tokenizer.json: vocab, unk token, casing ---
        let tj: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("tokenizer.json")).map_err(|e| format!("tokenizer.json: {e}"))?,
        )
        .map_err(|e| format!("tokenizer.json parse: {e}"))?;
        let model = &tj["model"];
        let vocab_obj = model["vocab"].as_object().ok_or("tokenizer.json has no model.vocab")?;
        let mut vocab = HashMap::with_capacity(vocab_obj.len());
        for (k, v) in vocab_obj {
            if let Some(id) = v.as_u64() {
                vocab.insert(k.clone(), id as u32);
            }
        }
        let unk_tok = model["unk_token"].as_str().unwrap_or("[UNK]");
        let unk = *vocab.get(unk_tok).unwrap_or(&0);
        let lowercase = tj["normalizer"]["lowercase"].as_bool().unwrap_or(true);

        // --- model.safetensors: the `embeddings` F32 [rows, dim] matrix ---
        let raw = std::fs::read(dir.join("model.safetensors")).map_err(|e| format!("model.safetensors: {e}"))?;
        if raw.len() < 8 {
            return Err("model.safetensors too small".into());
        }
        let hlen = u64::from_le_bytes(raw[0..8].try_into().unwrap()) as usize;
        let header_end = 8usize.checked_add(hlen).filter(|&e| e <= raw.len()).ok_or("bad safetensors header length")?;
        let hdr: serde_json::Value =
            serde_json::from_slice(&raw[8..header_end]).map_err(|e| format!("safetensors header: {e}"))?;
        let emb = hdr.get("embeddings").ok_or("safetensors has no 'embeddings' tensor")?;
        if emb["dtype"].as_str() != Some("F32") {
            return Err("embeddings dtype must be F32".into());
        }
        let shape = emb["shape"].as_array().ok_or("embeddings has no shape")?;
        let rows = shape.first().and_then(|v| v.as_u64()).ok_or("bad shape")? as usize;
        let dim = shape.get(1).and_then(|v| v.as_u64()).ok_or("bad shape")? as usize;
        let off = emb["data_offsets"].as_array().ok_or("no data_offsets")?;
        let s = off.first().and_then(|v| v.as_u64()).ok_or("bad data_offsets")? as usize;
        let e = off.get(1).and_then(|v| v.as_u64()).ok_or("bad data_offsets")? as usize;
        let data = raw
            .get(header_end + s..header_end + e)
            .ok_or("embeddings data out of bounds")?;
        if data.len() != rows * dim * 4 {
            return Err(format!("embeddings size mismatch: {} bytes for {rows}x{dim}", data.len()));
        }
        let matrix: Vec<f32> =
            data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();

        Ok(Self { vocab, matrix, rows, dim, unk, lowercase })
    }

    fn tokenize(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        for word in pretokenize(text, self.lowercase) {
            wordpiece(&word, &self.vocab, self.unk, &mut ids);
        }
        ids
    }
}

impl Embedder for StaticEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }
    fn name(&self) -> &str {
        "static-model2vec-v1"
    }
    fn embed(&self, text: &str) -> Vec<f32> {
        let ids = self.tokenize(text);
        let mut sum = vec![0f32; self.dim];
        let mut n = 0usize;
        for id in ids {
            let i = id as usize;
            if i < self.rows {
                let row = &self.matrix[i * self.dim..(i + 1) * self.dim];
                for (acc, &x) in sum.iter_mut().zip(row) {
                    *acc += x;
                }
                n += 1;
            }
        }
        if n > 0 {
            let inv = 1.0 / n as f32;
            for acc in sum.iter_mut() {
                *acc *= inv;
            }
        }
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for acc in sum.iter_mut() {
                *acc /= norm;
            }
        }
        sum
    }
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
}

fn push_char(ch: char, cur: &mut String, words: &mut Vec<String>) {
    if ch.is_whitespace() || ch.is_control() {
        if !cur.is_empty() {
            words.push(std::mem::take(cur));
        }
    } else if is_punct(ch) {
        if !cur.is_empty() {
            words.push(std::mem::take(cur));
        }
        words.push(ch.to_string());
    } else {
        cur.push(ch);
    }
}

/// BERT pre-tokenization: optional lowercase, then split on whitespace and isolate each
/// punctuation character as its own token.
fn pretokenize(text: &str, lowercase: bool) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch0 in text.chars() {
        if lowercase {
            for ch in ch0.to_lowercase() {
                push_char(ch, &mut cur, &mut words);
            }
        } else {
            push_char(ch0, &mut cur, &mut words);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Greedy longest-match WordPiece: split `word` into the longest in-vocab pieces, with `##`
/// on continuations; an unmatchable word becomes a single `[UNK]`.
fn wordpiece(word: &str, vocab: &HashMap<String, u32>, unk: u32, out: &mut Vec<u32>) {
    let chars: Vec<char> = word.chars().collect();
    if chars.is_empty() {
        return;
    }
    if chars.len() > 100 {
        out.push(unk);
        return;
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = chars.len();
        let mut found = None;
        while start < end {
            let sub: String = if start == 0 {
                chars[start..end].iter().collect()
            } else {
                let mut s = String::from("##");
                s.extend(chars[start..end].iter());
                s
            };
            if let Some(&id) = vocab.get(&sub) {
                found = Some(id);
                break;
            }
            end -= 1;
        }
        match found {
            Some(id) => {
                pieces.push(id);
                start = end;
            }
            None => {
                out.push(unk);
                return;
            }
        }
    }
    out.extend(pieces);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tiny_model(dir: &Path) {
        // vocab: [UNK]=0, rust=1, car=2, ##s=3
        let tok = serde_json::json!({
            "normalizer": { "type": "BertNormalizer", "lowercase": true },
            "model": { "type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##",
                "vocab": { "[UNK]": 0, "rust": 1, "car": 2, "##s": 3 } }
        });
        std::fs::write(dir.join("tokenizer.json"), tok.to_string()).unwrap();
        // 4 tokens x 2 dims, row-major f32 LE
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
    fn loads_tokenizes_and_embeds() {
        let dir = tempfile::tempdir().unwrap();
        write_tiny_model(dir.path());
        let e = StaticEmbedder::load(dir.path()).unwrap();
        assert_eq!(e.dim(), 2);

        // "Rust" lowercases to "rust" → id 1 → row [1,0], already unit length.
        let v = e.embed("Rust");
        assert!((v[0] - 1.0).abs() < 1e-5 && v[1].abs() < 1e-5, "got {v:?}");

        // "cars" → WordPiece "car" + "##s" → ids 2,3 → mean of [0,1] and [0.6,0.8],
        // then L2-normalized: a valid unit vector blending both pieces.
        let cars = e.embed("cars");
        let norm = (cars[0] * cars[0] + cars[1] * cars[1]).sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "not normalized: {cars:?}");
        assert!(cars[0] > 0.0 && cars[1] > 0.0, "both pieces should contribute: {cars:?}");
    }

    #[test]
    fn pretokenize_splits_punctuation() {
        assert_eq!(pretokenize("Hello, world!", true), vec!["hello", ",", "world", "!"]);
    }
}
