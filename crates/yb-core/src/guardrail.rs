//! Guardrail / fact-checking: validates a drafted answer against the knowledge
//! base and flags claims that lack supporting evidence (anti-hallucination).
//!
//! Pure-Rust and embedder-independent: grounding is measured by lexical overlap
//! between each claim and the evidence memories. The [`Validator`] trait is the
//! extension point for NLI/LLM-backed validators later.

use serde::Serialize;
use std::collections::HashSet;

use crate::memory::Memory;

/// Checks a drafted answer against evidence memories.
pub trait Validator: Send + Sync {
    fn validate(&self, answer: &str, evidence: &[Memory]) -> ValidationReport;
}

/// Per-claim grounding result.
#[derive(Debug, Clone, Serialize)]
pub struct ClaimCheck {
    pub text: String,
    pub supported: bool,
    pub score: f32,
    pub best_evidence_id: Option<String>,
}

/// Overall grounding report for an answer.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    /// Mean grounding score across claims (0..=1).
    pub grounding_score: f32,
    /// True when every claim is supported.
    pub grounded: bool,
    pub claims: Vec<ClaimCheck>,
    /// Claims whose grounding fell below the support threshold.
    pub unsupported: Vec<String>,
}

/// Lexical grounding validator: a claim is supported when a large-enough
/// fraction of its content words appear in some evidence memory.
pub struct RuleValidator {
    support_threshold: f32,
}

impl RuleValidator {
    pub fn new(support_threshold: f32) -> Self {
        Self { support_threshold }
    }
}

/// Very common words that carry little grounding signal.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "has", "have", "was",
    "with", "this", "that", "from", "they", "will", "would", "there", "their", "what", "which",
    "when", "your", "our", "its", "it's", "is", "of", "to", "in", "on", "a", "an", "as", "at",
    "be", "by", "or", "we", "us",
];

fn content_terms(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !STOPWORDS.contains(t))
        .map(|t| t.to_string())
        .collect()
}

fn split_claims(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let t = cur.trim();
            if t.len() > 1 {
                out.push(t.to_string());
            }
            cur.clear();
        }
    }
    let t = cur.trim();
    if t.len() > 1 {
        out.push(t.to_string());
    }
    out
}

impl Validator for RuleValidator {
    fn validate(&self, answer: &str, evidence: &[Memory]) -> ValidationReport {
        let evidence_terms: Vec<(String, HashSet<String>)> = evidence
            .iter()
            .map(|m| (m.id.clone(), content_terms(&m.content)))
            .collect();

        let claims_text = split_claims(answer);
        let mut claims = Vec::new();
        let mut unsupported = Vec::new();
        let mut score_sum = 0.0f32;

        for claim in &claims_text {
            let terms = content_terms(claim);
            let (mut best_score, mut best_id) = (0.0f32, None);
            if !terms.is_empty() {
                for (id, ev) in &evidence_terms {
                    let hits = terms.iter().filter(|t| ev.contains(*t)).count() as f32;
                    let frac = hits / terms.len() as f32;
                    if frac > best_score {
                        best_score = frac;
                        best_id = Some(id.clone());
                    }
                }
            } else {
                // A claim with no content terms (e.g. pure boilerplate) is treated
                // as trivially supported so it does not drag the score down.
                best_score = 1.0;
            }

            let supported = best_score >= self.support_threshold;
            if !supported {
                unsupported.push(claim.clone());
            }
            score_sum += best_score;
            claims.push(ClaimCheck {
                text: claim.clone(),
                supported,
                score: best_score,
                best_evidence_id: best_id,
            });
        }

        let grounding_score = if claims.is_empty() {
            0.0
        } else {
            score_sum / claims.len() as f32
        };
        ValidationReport {
            grounding_score,
            grounded: unsupported.is_empty() && !claims.is_empty(),
            claims,
            unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_memory;

    #[test]
    fn flags_unsupported_claim() {
        let evidence = vec![sample_memory(
            "The backend API is built with Rust using the Axum web framework.",
        )];
        let v = RuleValidator::new(0.4);
        let report = v.validate(
            "The backend uses Rust and Axum. It also runs on a blockchain ledger.",
            &evidence,
        );
        assert!(!report.grounded);
        assert!(report
            .unsupported
            .iter()
            .any(|c| c.to_lowercase().contains("blockchain")));
    }

    #[test]
    fn grounded_answer_passes() {
        let evidence = vec![sample_memory(
            "Authentication uses JWT tokens stored in Redis with a 15 minute expiry.",
        )];
        let v = RuleValidator::new(0.3);
        let report = v.validate("Authentication uses JWT tokens stored in Redis.", &evidence);
        assert!(report.grounded, "report: {report:?}");
    }
}
