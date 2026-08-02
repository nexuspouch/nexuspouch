//! Embedding providers for vector search (VECTOR_SEARCH_DESIGN.md P1).
//!
//! - Default local: deterministic hashing embedder (`local-hash-v0`) — offline,
//!   privacy-preserving interim until ONNX bge-small lands.
//! - Optional remote: OpenAI-compatible `/embeddings` via env.
//! - `NEXUSPOUCH_EMBED=off` → unavailable (semantic queries degrade to FTS5).

use serde_json::Value;
use std::sync::Arc;

pub trait Embedder: Send + Sync {
    fn name(&self) -> &str;
    fn dims(&self) -> usize;
    fn available(&self) -> bool {
        true
    }
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
}

/// Build the process-wide embedder from environment.
pub fn from_env() -> Arc<dyn Embedder> {
    let mode = std::env::var("NEXUSPOUCH_EMBED")
        .unwrap_or_default()
        .to_lowercase();
    if mode == "off" || mode == "0" || mode == "false" {
        return Arc::new(UnavailableEmbedder);
    }
    if let Ok(url) = std::env::var("NEXUSPOUCH_EMBED_URL") {
        if !url.trim().is_empty() {
            let token = std::env::var("NEXUSPOUCH_EMBED_TOKEN").unwrap_or_default();
            let model = std::env::var("NEXUSPOUCH_EMBED_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".into());
            return Arc::new(RemoteEmbedder {
                url: url.trim().to_string(),
                token,
                model,
                dims: std::env::var("NEXUSPOUCH_EMBED_DIMS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1536),
            });
        }
    }
    let dims = std::env::var("NEXUSPOUCH_EMBED_DIMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(HashingEmbedder::DEFAULT_DIMS);
    Arc::new(HashingEmbedder { dims })
}

pub struct UnavailableEmbedder;

impl Embedder for UnavailableEmbedder {
    fn name(&self) -> &str {
        "unavailable"
    }
    fn dims(&self) -> usize {
        0
    }
    fn available(&self) -> bool {
        false
    }
    fn embed(&self, _text: &str) -> Result<Vec<f32>, String> {
        Err("vector_unavailable".into())
    }
}

/// Local offline embedder: feature-hashing bag-of-tokens into a fixed vector.
/// Not a substitute for bge-small quality; keeps the pipeline and privacy story
/// working without a model download (ONNX swap-in later).
pub struct HashingEmbedder {
    pub dims: usize,
}

impl HashingEmbedder {
    pub const DEFAULT_DIMS: usize = 384;
    pub const NAME: &'static str = "local-hash-v0";
}

impl Embedder for HashingEmbedder {
    fn name(&self) -> &str {
        Self::NAME
    }
    fn dims(&self) -> usize {
        self.dims.max(8)
    }
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let dims = self.dims();
        let mut v = vec![0.0f32; dims];
        for tok in tokenize(text) {
            let h = fnv1a64(tok.as_bytes());
            let idx = (h as usize) % dims;
            let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
        l2_normalize(&mut v);
        Ok(v)
    }
}

pub struct RemoteEmbedder {
    pub url: String,
    pub token: String,
    pub model: String,
    pub dims: usize,
}

impl Embedder for RemoteEmbedder {
    fn name(&self) -> &str {
        "remote-openai-compat"
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });
        let mut req = ureq::post(&self.url).set("Content-Type", "application/json");
        if !self.token.is_empty() {
            req = req.set("Authorization", &format!("Bearer {}", self.token));
        }
        let resp = req
            .send_string(&body.to_string())
            .map_err(|e| format!("embed http: {e}"))?;
        let raw = resp
            .into_string()
            .map_err(|e| format!("embed body: {e}"))?;
        let val: Value =
            serde_json::from_str(&raw).map_err(|e| format!("embed json: {e}"))?;
        let arr = val
            .pointer("/data/0/embedding")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "embed response missing data[0].embedding".to_string())?;
        let mut out = Vec::with_capacity(arr.len());
        for x in arr {
            out.push(x.as_f64().unwrap_or(0.0) as f32);
        }
        if out.is_empty() {
            return Err("empty embedding".into());
        }
        l2_normalize(&mut out);
        Ok(out)
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
    }
    dot
}

pub fn packing(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn unpacking(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && (c as u32) < 0x80)
        .filter(|s| !s.is_empty())
        .flat_map(|s| {
            // Also emit CJK unigrams for zh text.
            if s.chars().any(|c| (c as u32) > 0x2e7f) {
                s.chars()
                    .filter(|c| !c.is_whitespace())
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
            } else {
                vec![s.to_string()]
            }
        })
        .collect()
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn l2_normalize(v: &mut [f32]) {
    let mut sum = 0.0f32;
    for x in v.iter() {
        sum += *x * *x;
    }
    if sum <= 0.0 {
        return;
    }
    let inv = 1.0 / sum.sqrt();
    for x in v.iter_mut() {
        *x *= inv;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_embed_is_deterministic_and_normalized() {
        let e = HashingEmbedder {
            dims: HashingEmbedder::DEFAULT_DIMS,
        };
        let a = e.embed("hello vector world").unwrap();
        let b = e.embed("hello vector world").unwrap();
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3 || norm == 0.0);
        let c = e.embed("totally different phrase about cats").unwrap();
        assert!(cosine(&a, &c) < cosine(&a, &b) + 0.01);
    }

    #[test]
    fn unavailable_rejects() {
        let e = UnavailableEmbedder;
        assert!(!e.available());
        assert!(e.embed("x").is_err());
    }
}
