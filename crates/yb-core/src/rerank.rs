//! Lexical (BM25-style) reranking applied on top of the hybrid RRF + metadata
//! score, to sharpen query relevance of the top results.
//!
//! Pure-Rust and embedder-independent. The [`Reranker`] trait is the extension
//! point: a cross-encoder or LLM reranker can be dropped in behind it later
//! without changing [`crate::brain::Brain`].

use std::collections::HashMap;

use crate::search::ScoredMemory;

/// Reorders scored candidates by relevance to the query.
pub trait Reranker: Send + Sync {
    fn rerank(&self, query: &str, candidates: Vec<ScoredMemory>) -> Vec<ScoredMemory>;
}

/// Okapi BM25 over candidate documents, blended with the incoming fused score.
pub struct LexicalReranker {
    /// Blend weight of the lexical score vs the incoming score (0..=1).
    lexical_weight: f32,
}

impl LexicalReranker {
    pub fn new(lexical_weight: f32) -> Self {
        Self {
            lexical_weight: lexical_weight.clamp(0.0, 1.0),
        }
    }
}

/// Lowercase alphanumeric tokens with length > 1 (drops punctuation/stopword-ish
/// single chars). Kept intentionally simple and deterministic.
fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_string())
        .collect()
}

impl Reranker for LexicalReranker {
    fn rerank(&self, query: &str, candidates: Vec<ScoredMemory>) -> Vec<ScoredMemory> {
        let q_terms = tokenize(query);
        if q_terms.is_empty() || candidates.len() < 2 {
            return candidates;
        }

        // Build per-document token lists from content + headline + tags.
        let docs: Vec<Vec<String>> = candidates
            .iter()
            .map(|c| {
                let mut t = tokenize(&c.memory.content);
                t.extend(tokenize(&c.memory.headline));
                for tag in &c.memory.tags {
                    t.extend(tokenize(tag));
                }
                t
            })
            .collect();

        let n = docs.len() as f32;
        let avgdl = (docs.iter().map(|d| d.len()).sum::<usize>() as f32 / n).max(1.0);

        // Document frequency per query term across the candidate set.
        let mut df: HashMap<&str, f32> = HashMap::new();
        for term in &q_terms {
            let count = docs.iter().filter(|d| d.iter().any(|w| w == term)).count() as f32;
            df.insert(term.as_str(), count);
        }

        const K1: f32 = 1.2;
        const B: f32 = 0.75;
        let mut lex_scores = Vec::with_capacity(docs.len());
        for d in &docs {
            let dl = d.len() as f32;
            let mut tf: HashMap<&str, f32> = HashMap::new();
            for w in d {
                *tf.entry(w.as_str()).or_insert(0.0) += 1.0;
            }
            let mut score = 0.0f32;
            for term in &q_terms {
                let f = *tf.get(term.as_str()).unwrap_or(&0.0);
                if f == 0.0 {
                    continue;
                }
                let nq = *df.get(term.as_str()).unwrap_or(&0.0);
                let idf = (((n - nq + 0.5) / (nq + 0.5)) + 1.0).ln();
                let denom = f + K1 * (1.0 - B + B * dl / avgdl);
                score += idf * (f * (K1 + 1.0)) / denom.max(1e-6);
            }
            lex_scores.push(score);
        }

        // Normalize both signals to 0..1 before blending so weights are meaningful.
        let max_lex = lex_scores.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
        let max_base = candidates
            .iter()
            .map(|c| c.score)
            .fold(0.0f32, f32::max)
            .max(1e-6);
        let w = self.lexical_weight;

        let mut out: Vec<ScoredMemory> = candidates
            .into_iter()
            .enumerate()
            .map(|(i, mut sm)| {
                let lex = lex_scores[i] / max_lex;
                let base = sm.score / max_base;
                sm.score = w * lex + (1.0 - w) * base;
                sm
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_memory;

    fn scored(content: &str, base: f32) -> ScoredMemory {
        ScoredMemory {
            memory: sample_memory(content),
            score: base,
        }
    }

    #[test]
    fn lexical_match_ranks_first() {
        // The last candidate has the highest incoming base score but no lexical
        // overlap; the reranker should promote the query-matching document.
        let cands = vec![
            scored("kubernetes deployment on GCP with helm charts", 0.9),
            scored("the auth service uses JWT tokens stored in Redis", 0.3),
        ];
        let rr = LexicalReranker::new(0.7);
        let out = rr.rerank("how does JWT auth work with Redis", cands);
        assert!(
            out[0].memory.content.contains("JWT"),
            "expected JWT doc first, got {:?}",
            out[0].memory.content
        );
    }

    #[test]
    fn empty_query_is_noop() {
        let cands = vec![scored("a", 0.5), scored("b", 0.4)];
        let out = LexicalReranker::new(0.5).rerank("", cands);
        assert_eq!(out.len(), 2);
    }
}
