//! Embedding abstraction.
//!
//! The engine depends only on the [`Embedder`] trait. The default backend is a
//! dependency-free, deterministic feature-hashing embedder so the whole system
//! builds and runs on any machine without native ONNX Runtime. A real ONNX
//! backend (`nomic-embed-text-v1.5`, etc.) can be added behind a feature flag
//! and dropped in via this same trait — nothing else needs to change.

use std::sync::Arc;

/// Produces dense vector embeddings for text.
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
