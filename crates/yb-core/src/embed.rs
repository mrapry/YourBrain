//! Embedding abstraction.
//!
//! The engine depends only on the [`Embedder`] trait. The default backend is a
//! dependency-free, deterministic feature-hashing embedder so the whole system
//! builds and runs on any machine without native ONNX Runtime. A real ONNX
//! backend (`nomic-embed-text-v1.5`, etc.) can be added behind a feature flag
//! and dropped in via this same trait — nothing else needs to change.

use std::sync::Arc;

/// Produces dense vector embeddings for text.
///
/// Some models (e.g. the E5 family) are asymmetric: they expect a `query:` /
/// `passage:` instruction prefix and score best when queries and stored
/// documents are embedded differently. [`Embedder::embed_query`] and
/// [`Embedder::embed_document`] express that intent; both default to
/// [`Embedder::embed`], so symmetric backends (like [`HashEmbedder`]) need not
/// implement them.
pub trait Embedder: Send + Sync {
    /// Stable identifier of the model (stored in `brain_meta`, see ADR-5).
    fn model_id(&self) -> &str;
    /// Output vector dimension. Locked into the DB at creation time.
    fn dimension(&self) -> usize;
    /// Embed a single string into an L2-normalized vector.
    fn embed(&self, text: &str) -> Vec<f32>;
    /// Embed many strings. Default maps over [`Embedder::embed`].
    fn embed_batch(&self, texts: &[String]) -> Vec<Vec<f32>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
    /// Embed a search query. Defaults to [`Embedder::embed`].
    fn embed_query(&self, text: &str) -> Vec<f32> {
        self.embed(text)
    }
    /// Embed a stored document/passage. Defaults to [`Embedder::embed`].
    fn embed_document(&self, text: &str) -> Vec<f32> {
        self.embed(text)
    }
}

impl Embedder for Arc<dyn Embedder> {
    fn model_id(&self) -> &str {
        (**self).model_id()
    }
    fn dimension(&self) -> usize {
        (**self).dimension()
    }
    fn embed(&self, text: &str) -> Vec<f32> {
        (**self).embed(text)
    }
    fn embed_query(&self, text: &str) -> Vec<f32> {
        (**self).embed_query(text)
    }
    fn embed_document(&self, text: &str) -> Vec<f32> {
        (**self).embed_document(text)
    }
}

/// Deterministic, dependency-free embedder based on the feature-hashing trick.
///
/// Tokens (unigrams + bigrams) are hashed into `dim` buckets with signed
/// contributions, then the vector is L2-normalized. Texts with overlapping
/// vocabulary get high cosine similarity — sufficient for candidate search,
/// near-duplicate detection, and deterministic tests.
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "embedding dimension must be > 0");
        Self { dim }
    }
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self::new(256)
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|s| s.to_string())
        .collect()
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> &str {
        "hash-bow-v1"
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        let tokens = tokenize(text);

        let mut add = |token: &str| {
            let h = fnv1a(token.as_bytes());
            let idx = (h % self.dim as u64) as usize;
            // Sign hashing reduces collisions' bias.
            let sign = if (h >> 63) & 1 == 1 { 1.0 } else { -1.0 };
            v[idx] += sign;
        };

        for w in &tokens {
            add(w);
        }
        // Bigrams capture a little word order / phrase structure.
        for pair in tokens.windows(2) {
            add(&format!("{}_{}", pair[0], pair[1]));
        }

        l2_normalize(&mut v);
        v
    }
}

/// ONNX sentence-transformer backend (feature `onnx`), powered by `fastembed`.
///
/// `fastembed` handles tokenization, mean pooling, and L2 normalization, and
/// downloads the model from HuggingFace on first use (cached on disk). Models
/// in the E5 family are asymmetric, so `query:` / `passage:` prefixes are
/// applied automatically via [`Embedder::embed_query`] / `embed_document`.
#[cfg(feature = "onnx")]
pub struct OnnxEmbedder {
    inner: fastembed::TextEmbedding,
    model_id: String,
    dim: usize,
    /// Whether to prepend E5-style `query:` / `passage:` instruction prefixes.
    prefixed: bool,
}

#[cfg(feature = "onnx")]
impl OnnxEmbedder {
    /// Build the embedder for a model key (e.g. `"multilingual-e5-small"`),
    /// downloading it into `cache_dir` if provided. Returns the resolved
    /// dimension so callers can keep the DB lock (ADR-5) consistent.
    pub fn new(model_key: &str, cache_dir: Option<&str>) -> crate::error::Result<Self> {
        use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

        let key = model_key.trim().to_lowercase();
        let (model, dim) = match key.as_str() {
            "multilingual-e5-small" | "e5-small" => (EmbeddingModel::MultilingualE5Small, 384),
            "multilingual-e5-base" | "e5-base" => (EmbeddingModel::MultilingualE5Base, 768),
            "multilingual-e5-large" | "e5-large" => (EmbeddingModel::MultilingualE5Large, 1024),
            "all-minilm-l6-v2" | "minilm" => (EmbeddingModel::AllMiniLML6V2, 384),
            "bge-small-en-v1.5" | "bge-small-en" | "bge-small" => {
                (EmbeddingModel::BGESmallENV15, 384)
            }
            "paraphrase-multilingual-minilm-l12-v2" | "paraphrase-ml-minilm" => {
                (EmbeddingModel::ParaphraseMLMiniLML12V2, 384)
            }
            other => {
                return Err(crate::error::YbError::Embedder(format!(
                    "unsupported ONNX model `{other}` (try: multilingual-e5-small, \
                     multilingual-e5-base, all-minilm-l6-v2, bge-small-en-v1.5, \
                     paraphrase-multilingual-minilm-l12-v2)"
                )));
            }
        };

        let mut opts = InitOptions::new(model).with_show_download_progress(true);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(std::path::PathBuf::from(dir));
        }
        let inner = TextEmbedding::try_new(opts)
            .map_err(|e| crate::error::YbError::Embedder(format!("failed to load model: {e}")))?;

        Ok(Self {
            inner,
            model_id: key.clone(),
            dim,
            // E5 models are trained with instruction prefixes; others are not.
            prefixed: key.contains("e5"),
        })
    }

    fn run(&self, text: &str, prefix: &str) -> Vec<f32> {
        let input = if self.prefixed {
            format!("{prefix}{text}")
        } else {
            text.to_string()
        };
        match self.inner.embed(vec![input], None) {
            Ok(mut v) if !v.is_empty() => v.swap_remove(0),
            _ => vec![0f32; self.dim],
        }
    }

    fn run_batch(&self, texts: Vec<String>) -> Vec<Vec<f32>> {
        self.inner.embed(texts, None).unwrap_or_default()
    }
}

#[cfg(feature = "onnx")]
impl Embedder for OnnxEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        // Generic embed is treated as a document/passage.
        self.run(text, "passage: ")
    }

    fn embed_query(&self, text: &str) -> Vec<f32> {
        self.run(text, "query: ")
    }

    fn embed_document(&self, text: &str) -> Vec<f32> {
        self.run(text, "passage: ")
    }

    fn embed_batch(&self, texts: &[String]) -> Vec<Vec<f32>> {
        if !self.prefixed {
            return self.run_batch(texts.to_vec());
        }
        let prefixed: Vec<String> = texts.iter().map(|t| format!("passage: {t}")).collect();
        self.run_batch(prefixed)
    }
}

/// Normalize a vector to unit L2 length in place. Zero vectors are left as-is.
pub fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity between two equal-length vectors. Assumes (but does not
/// require) L2-normalized inputs.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < f32::EPSILON || nb < f32::EPSILON {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_and_norm() {
        let e = HashEmbedder::new(128);
        let v = e.embed("hello world");
        assert_eq!(v.len(), 128);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4 || norm == 0.0);
    }

    #[test]
    fn similar_texts_score_higher() {
        let e = HashEmbedder::default();
        let a = e.embed("authentication uses JWT tokens in Redis");
        let b = e.embed("auth with JWT tokens stored in Redis");
        let c = e.embed("the weather today is sunny and warm");
        let sim_ab = cosine(&a, &b);
        let sim_ac = cosine(&a, &c);
        assert!(sim_ab > sim_ac, "sim_ab={sim_ab} sim_ac={sim_ac}");
    }

    #[test]
    fn deterministic() {
        let e = HashEmbedder::default();
        assert_eq!(e.embed("same input"), e.embed("same input"));
    }
}
