//! Embedding providers for vector search (VECTOR_SEARCH_DESIGN.md P1).
//!
//! - Default local: ONNX via `fastembed` (`bge-small-zh-v1.5`); first run may
//!   download the model into the cache dir. On load failure → `local-hash-v0`.
//! - `NEXUSPOUCH_EMBED=hash` forces the hashing embedder (tests / offline CI).
//! - Optional remote: OpenAI-compatible `/embeddings` via `NEXUSPOUCH_EMBED_URL`.
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
    if mode == "hash" {
        return hashing_from_env();
    }

    #[cfg(feature = "onnx")]
    {
        // Tests stay offline by default; set NEXUSPOUCH_EMBED=onnx to exercise ONNX.
        let try_onnx = mode == "onnx"
            || mode == "local"
            || mode == "auto"
            || (mode.is_empty() && !cfg!(test));
        if try_onnx {
            match OnnxEmbedder::try_from_env() {
                Ok(e) => {
                    tracing::info!(embedder = e.name(), dims = e.dims(), "onnx embedder ready");
                    return Arc::new(e);
                }
                Err(err) => {
                    tracing::warn!(%err, "onnx embedder unavailable; falling back to local-hash-v0");
                }
            }
        }
    }

    #[cfg(not(feature = "onnx"))]
    if mode == "onnx" || mode == "local" {
        tracing::warn!("built without `onnx` feature; using local-hash-v0");
    }

    hashing_from_env()
}

fn hashing_from_env() -> Arc<dyn Embedder> {
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
/// Fallback when ONNX is disabled, fails to load, or tests force `hash`.
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

#[cfg(feature = "onnx")]
mod onnx {
    use super::{l2_normalize, Embedder};
    use fastembed::{
        EmbeddingModel, InitOptionsUserDefined, Pooling, TextEmbedding, TextInitOptions,
        TokenizerFiles, UserDefinedEmbeddingModel,
    };
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::sync::Mutex;

    /// ONNX local embedder (fastembed / ort). Default model: BGE small zh v1.5.
    pub struct OnnxEmbedder {
        name: String,
        dims: usize,
        model: Mutex<TextEmbedding>,
    }

    impl OnnxEmbedder {
        pub fn try_from_env() -> Result<Self, String> {
            // Prefer a pre-fetched directory (offline / mirror / air-gapped).
            // Layout: model.onnx + tokenizer.json + config.json +
            // special_tokens_map.json + tokenizer_config.json
            // (onnx/model.onnx also accepted).
            if let Ok(dir) = std::env::var("NEXUSPOUCH_EMBED_MODEL_DIR") {
                let dir = dir.trim();
                if !dir.is_empty() {
                    return Self::from_model_dir(Path::new(dir));
                }
            }
            let model = resolve_model()?;
            let dims = TextEmbedding::get_model_info(&model)
                .map(|m| m.dim)
                .map_err(|e| e.to_string())?;
            let mut opts = TextInitOptions::new(model.clone()).with_show_download_progress(true);
            if let Ok(cache) = std::env::var("NEXUSPOUCH_EMBED_CACHE") {
                let p = PathBuf::from(cache.trim());
                if !p.as_os_str().is_empty() {
                    opts = opts.with_cache_dir(p);
                }
            }
            if let Ok(threads) = std::env::var("NEXUSPOUCH_EMBED_THREADS") {
                if let Ok(n) = threads.parse::<usize>() {
                    opts = opts.with_intra_threads(n.max(1));
                }
            }
            let text = TextEmbedding::try_new(opts).map_err(|e| {
                format!(
                    "onnx init: {e} (tip: set HF_ENDPOINT or NEXUSPOUCH_EMBED_MODEL_DIR for offline/mirror)"
                )
            })?;
            Ok(Self {
                name: format!("local-onnx:{model}"),
                dims,
                model: Mutex::new(text),
            })
        }

        pub fn from_model_dir(dir: &Path) -> Result<Self, String> {
            let onnx_path = [
                dir.join("model.onnx"),
                dir.join("onnx").join("model.onnx"),
                dir.join("model_optimized.onnx"),
            ]
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| format!("onnx model missing under {}", dir.display()))?;
            let read = |name: &str| {
                std::fs::read(dir.join(name))
                    .map_err(|e| format!("read {}: {e}", dir.join(name).display()))
            };
            let user = UserDefinedEmbeddingModel::new(
                std::fs::read(&onnx_path)
                    .map_err(|e| format!("read {}: {e}", onnx_path.display()))?,
                TokenizerFiles {
                    tokenizer_file: read("tokenizer.json")?,
                    config_file: read("config.json")?,
                    special_tokens_map_file: read("special_tokens_map.json")?,
                    tokenizer_config_file: read("tokenizer_config.json")?,
                },
            )
            .with_pooling(Pooling::Cls);
            let mut opts = InitOptionsUserDefined::new();
            if let Ok(threads) = std::env::var("NEXUSPOUCH_EMBED_THREADS") {
                if let Ok(n) = threads.parse::<usize>() {
                    opts = opts.with_intra_threads(n.max(1));
                }
            }
            let text = TextEmbedding::try_new_from_user_defined(user, opts)
                .map_err(|e| format!("onnx local dir init: {e}"))?;
            let dims = std::env::var("NEXUSPOUCH_EMBED_DIMS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| {
                    // Probe once to discover dims when not set.
                    0
                });
            let mut emb = Self {
                name: format!("local-onnx:dir:{}", dir.display()),
                dims: dims.max(1),
                model: Mutex::new(text),
            };
            if dims == 0 {
                let probe = emb.embed("probe")?;
                emb.dims = probe.len();
            }
            Ok(emb)
        }
    }

    fn resolve_model() -> Result<EmbeddingModel, String> {
        let raw = std::env::var("NEXUSPOUCH_EMBED_MODEL").unwrap_or_default();
        let s = raw.trim();
        if s.is_empty() {
            // Product default: Chinese-capable small BGE (design §3.3).
            return Ok(EmbeddingModel::BGESmallZHV15);
        }
        let lower = s.to_lowercase();
        match lower.as_str() {
            "bge-small-zh" | "bge-small-zh-v1.5" | "zh" | "bgesmallzhv15" => {
                Ok(EmbeddingModel::BGESmallZHV15)
            }
            "bge-small-en" | "bge-small-en-v1.5" | "en" | "bgesmallenv15" => {
                Ok(EmbeddingModel::BGESmallENV15)
            }
            "bge-small-en-q" | "bgesmallenv15q" => Ok(EmbeddingModel::BGESmallENV15Q),
            "multilingual-e5-small" | "e5-small" | "multilinguale5small" => {
                Ok(EmbeddingModel::MultilingualE5Small)
            }
            _ => EmbeddingModel::from_str(s),
        }
    }

    impl Embedder for OnnxEmbedder {
        fn name(&self) -> &str {
            &self.name
        }
        fn dims(&self) -> usize {
            self.dims
        }
        fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
            let mut guard = self
                .model
                .lock()
                .map_err(|_| "onnx embedder lock poisoned".to_string())?;
            let mut out = guard
                .embed(vec![text], None)
                .map_err(|e| format!("onnx embed: {e}"))?;
            let mut v = out.pop().ok_or_else(|| "onnx embed empty".to_string())?;
            l2_normalize(&mut v);
            Ok(v)
        }
    }
}

#[cfg(feature = "onnx")]
pub use onnx::OnnxEmbedder;

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

    #[test]
    fn from_env_defaults_to_hash_under_test() {
        // cfg(test) path: empty NEXUSPOUCH_EMBED → hash (no model download).
        std::env::remove_var("NEXUSPOUCH_EMBED");
        std::env::remove_var("NEXUSPOUCH_EMBED_URL");
        let e = from_env();
        assert_eq!(e.name(), HashingEmbedder::NAME);
    }

    #[test]
    #[ignore = "needs ONNX model: set NEXUSPOUCH_EMBED_MODEL_DIR or allow HF download"]
    fn onnx_embedder_smoke() {
        #[cfg(feature = "onnx")]
        {
            std::env::remove_var("NEXUSPOUCH_EMBED_URL");
            let e = OnnxEmbedder::try_from_env().expect("onnx load");
            assert!(e.name().starts_with("local-onnx:"));
            assert!(e.dims() >= 384);
            let a = e.embed("小猫喜欢喝牛奶").unwrap();
            let b = e.embed("猫咪爱喝奶").unwrap();
            let c = e.embed("quantum chromodynamics lattice").unwrap();
            assert!(cosine(&a, &b) > cosine(&a, &c));
        }
        #[cfg(not(feature = "onnx"))]
        {
            panic!("rebuild with --features onnx");
        }
    }
}
