//! Conflict detection and resolution.
//!
//! Implements the tiered model from ADR-3. Tier 1 (rule-based, zero
//! dependency) is implemented here and always available. Tiers 2 (bundled NLI)
//! and 3 (external LLM) plug in behind the [`Judge`] trait — the rule engine is
//! the guaranteed fallback.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::Memory;

/// A candidate existing memory returned by vector search, with its similarity
/// to the incoming memory.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub memory: Memory,
    pub similarity: f32,
}

/// The relationship between a new memory and an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictRelation {
    /// No meaningful relation.
    Unrelated,
    /// Adds new information about the same topic.
    Complementary,
    /// Says essentially the same thing.
    Duplicate,
    /// Updates/replaces the old memory.
    Supersede,
    /// Mutually exclusive with the old memory.
    Contradicts,
}

impl ConflictRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictRelation::Unrelated => "unrelated",
            ConflictRelation::Complementary => "complementary",
            ConflictRelation::Duplicate => "duplicate",
            ConflictRelation::Supersede => "supersede",
            ConflictRelation::Contradicts => "contradicts",
        }
    }
}

/// The action suggested by the analysis (advisory, not binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedAction {
    Store,
    Merge,
    Replace,
    AskUser,
    Skip,
}

/// The result of analyzing a new memory against a set of candidates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictAnalysis {
    pub relation: ConflictRelation,
    pub confidence: f32,
    pub reasoning: String,
    #[serde(default)]
    pub key_difference: Option<String>,
    pub suggested_action: SuggestedAction,
    /// The primary existing memory this analysis concerns.
    pub existing_id: String,
    /// Which tier produced this analysis (for observability).
    pub tier: u8,
}

/// Lifecycle state of a pending conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictState {
    Pending,
    Resolved,
    Expired,
    AutoResolved,
}

impl ConflictState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictState::Pending => "pending",
            ConflictState::Resolved => "resolved",
            ConflictState::Expired => "expired",
            ConflictState::AutoResolved => "auto_resolved",
        }
    }
}

/// The action a user (or auto-resolver) chose to resolve a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionAction {
    /// New replaces old (archive existing).
    Replace,
    /// Both are valid (different context).
    KeepBoth,
    /// Existing is still correct; drop the new one.
    DiscardNew,
    /// Combine into one updated memory.
    Merge,
}

/// A resolution decision for a conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolution {
    pub action: ResolutionAction,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub merged_content: Option<String>,
}

/// A stored, pending (or resolved) conflict. The full new memory is preserved
/// as a struct so nothing is lost on resolve (see ADR-7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub new_memory: Memory,
    pub existing_memory_ids: Vec<String>,
    pub analysis: ConflictAnalysis,
    pub state: ConflictState,
    #[serde(default)]
    pub resolution: Option<Resolution>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub resolved_by: Option<String>,
}

/// The outcome of running conflict detection for an incoming memory.
#[derive(Debug, Clone)]
pub enum ConflictOutcome {
    /// No conflict — store the memory directly.
    None,
    /// A conflict that can be resolved automatically with high confidence.
    AutoResolved {
        analysis: ConflictAnalysis,
        action: ResolutionAction,
    },
    /// A conflict that needs user review.
    NeedsReview(ConflictAnalysis),
}

/// Thresholds controlling auto-resolution behaviour (mirrors `[conflict]` config).
#[derive(Debug, Clone)]
pub struct ConflictThresholds {
    pub auto_supersede: f32,
    pub auto_duplicate: f32,
    pub auto_complement: f32,
}

impl Default for ConflictThresholds {
    fn default() -> Self {
        Self {
            auto_supersede: 0.92,
            auto_duplicate: 0.95,
            auto_complement: 0.80,
        }
    }
}

/// Pluggable relationship classifier. Tier 2 (NLI) and Tier 3 (LLM) implement
/// this; the built-in rule engine is used when no judge is configured.
pub trait Judge: Send + Sync {
    /// Classify the relationship between `new` and the best `candidate`.
    fn classify(&self, new: &Memory, candidate: &Candidate) -> Option<ConflictAnalysis>;
}

/// Tier 1 rule-based conflict detector. Deliberately high-precision: it prefers
/// to miss a conflict rather than false-alarm the user (see ADR-10).
#[derive(Debug, Default, Clone)]
pub struct RuleEngine;

impl RuleEngine {
    pub fn new() -> Self {
        RuleEngine
    }

    /// Analyze `new` against candidates and return the single most significant
    /// relationship, if any.
    ///
    /// Candidates are assumed to have already passed the vector-similarity
    /// threshold upstream (see [`crate::brain::Brain`]). The rules below rely on
    /// embedder-independent textual signals so behaviour is stable regardless of
    /// which embedding backend is active. High precision by design (ADR-10):
    /// when in doubt, return `None` and let the memory be stored normally.
    pub fn analyze(&self, new: &Memory, candidates: &[Candidate]) -> Option<ConflictAnalysis> {
        for c in candidates {
            let existing = &c.memory;

            // 1. Near-exact duplicate (token-set overlap on compressed text).
            if text_similarity(&new.compressed, &existing.compressed) > 0.90 {
                return Some(ConflictAnalysis {
                    relation: ConflictRelation::Duplicate,
                    confidence: 0.95,
                    reasoning: "New memory is near-identical to an existing one.".into(),
                    key_difference: None,
                    suggested_action: SuggestedAction::Skip,
                    existing_id: existing.id.clone(),
                    tier: 1,
                });
            }

            // 2. Same author + newer + explicit supersede signal.
            if new.author == existing.author
                && new.created_at >= existing.created_at
                && contains_supersede_signal(&new.content)
            {
                return Some(ConflictAnalysis {
                    relation: ConflictRelation::Supersede,
                    confidence: 0.80,
                    reasoning: "Same author updated the same topic with a change signal.".into(),
                    key_difference: None,
                    suggested_action: SuggestedAction::Replace,
                    existing_id: existing.id.clone(),
                    tier: 1,
                });
            }

            // 3. Negation / contradiction signal.
            if contains_negation(&new.content, &existing.content) {
                return Some(ConflictAnalysis {
                    relation: ConflictRelation::Contradicts,
                    confidence: 0.75,
                    reasoning: "New memory negates a claim in the existing one.".into(),
                    key_difference: None,
                    suggested_action: SuggestedAction::AskUser,
                    existing_id: existing.id.clone(),
                    tier: 1,
                });
            }

            // 4. Competing claim: same subject+verb, different object.
            if let Some(diff) = competing_claim(&new.content, &existing.content) {
                return Some(ConflictAnalysis {
                    relation: ConflictRelation::Supersede,
                    confidence: 0.70,
                    reasoning: "Same subject with a different value — likely an update.".into(),
                    key_difference: Some(diff),
                    suggested_action: SuggestedAction::AskUser,
                    existing_id: existing.id.clone(),
                    tier: 1,
                });
            }
        }
        None
    }
}

/// Decide whether an analysis can be resolved automatically.
///
/// Contradictions are NEVER auto-resolved; cross-author supersedes always ask.
pub fn should_auto_resolve(
    analysis: &ConflictAnalysis,
    new: &Memory,
    existing: &Memory,
    thresholds: &ConflictThresholds,
) -> Option<ResolutionAction> {
    match analysis.relation {
        ConflictRelation::Duplicate if analysis.confidence >= thresholds.auto_duplicate => {
            Some(ResolutionAction::DiscardNew)
        }
        ConflictRelation::Complementary if analysis.confidence >= thresholds.auto_complement => {
            Some(ResolutionAction::KeepBoth)
        }
        ConflictRelation::Supersede
            if analysis.confidence >= thresholds.auto_supersede
                && new.author == existing.author =>
        {
            Some(ResolutionAction::Replace)
        }
        // Contradictions and cross-author supersedes always require review.
        _ => None,
    }
}

/// Compute the expiry timestamp for a new conflict given a TTL in seconds.
pub fn expiry_from(created_at: DateTime<Utc>, ttl_secs: i64) -> DateTime<Utc> {
    created_at + Duration::seconds(ttl_secs)
}

// ----------------------------------------------------------------------------
// Rule helpers
// ----------------------------------------------------------------------------

const SUPERSEDE_SIGNALS: &[&str] = &[
    "sekarang",
    "now",
    "migrasi",
    "migrate",
    "pindah ke",
    "ganti",
    "replace",
    "update",
    "ubah",
    "change to",
    "mulai pakai",
    "switched to",
    "moved to",
    "upgraded to",
    "no longer",
    "tidak lagi",
    "beralih",
];

fn contains_supersede_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    SUPERSEDE_SIGNALS.iter().any(|s| lower.contains(s))
}

const NEGATION_PAIRS: &[(&str, &str)] = &[
    ("pakai", "tidak pakai"),
    ("enable", "disable"),
    ("enabled", "disabled"),
    ("tambah", "hapus"),
    ("aktif", "nonaktif"),
    ("use", "don't use"),
    ("uses", "does not use"),
];

fn contains_negation(new_text: &str, old_text: &str) -> bool {
    let new_l = new_text.to_lowercase();
    let old_l = old_text.to_lowercase();
    for (pos, neg) in NEGATION_PAIRS {
        if (new_l.contains(neg) && old_l.contains(pos) && !old_l.contains(neg))
            || (old_l.contains(neg) && new_l.contains(pos) && !new_l.contains(neg))
        {
            return true;
        }
    }
    false
}

/// A minimal claim: (subject, verb, object). Only the technical patterns from
/// ADR-10 Phase 1 are recognized.
#[derive(Debug, PartialEq)]
struct Claim {
    subject: String,
    verb: String,
    object: String,
}

fn extract_claims(text: &str) -> Vec<Claim> {
    let mut claims = Vec::new();
    let lower = text.to_lowercase();
    let verbs = [
        (
            "uses",
            ["pakai", "menggunakan", "memakai", "use", "using"].as_slice(),
        ),
        ("is", ["adalah", "yaitu", "is", "are"].as_slice()),
        (
            "deployed_on",
            ["deploy di", "hosted on", "runs on", "di-deploy di"].as_slice(),
        ),
    ];
    let words: Vec<&str> = lower.split_whitespace().collect();
    for (canonical, aliases) in verbs {
        for alias in aliases {
            if let Some(pos) = lower.find(alias) {
                // subject = last word before the verb; object = first meaningful word after.
                let before = &lower[..pos];
                let after = &lower[pos + alias.len()..];
                let subject = before.split_whitespace().last().unwrap_or("").to_string();
                let object = after
                    .split_whitespace()
                    .find(|w| w.len() > 1)
                    .unwrap_or("")
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if !subject.is_empty() && !object.is_empty() {
                    claims.push(Claim {
                        subject,
                        verb: canonical.to_string(),
                        object,
                    });
                }
            }
        }
    }
    let _ = words;
    claims
}

/// If both texts assert the same subject+verb with a different object, return a
/// human-readable description of the difference.
fn competing_claim(new_text: &str, old_text: &str) -> Option<String> {
    let new_claims = extract_claims(new_text);
    let old_claims = extract_claims(old_text);
    for n in &new_claims {
        for o in &old_claims {
            if n.subject == o.subject && n.verb == o.verb && n.object != o.object {
                return Some(format!(
                    "{} {} '{}' vs '{}'",
                    n.subject, n.verb, o.object, n.object
                ));
            }
        }
    }
    None
}

/// Token-set Jaccard similarity used for near-duplicate detection. Cheap and
/// language-agnostic.
pub fn text_similarity(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;
    let sa: HashSet<&str> = a.split_whitespace().collect();
    let sb: HashSet<&str> = b.split_whitespace().collect();
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supersede_signal_detected() {
        assert!(contains_supersede_signal(
            "Deployment sekarang pakai Docker Swarm"
        ));
        assert!(contains_supersede_signal("We migrated to Postgres"));
        assert!(!contains_supersede_signal("Auth uses JWT"));
    }

    #[test]
    fn negation_detected() {
        assert!(contains_negation(
            "kita tidak pakai Redis",
            "kita pakai Redis"
        ));
        assert!(contains_negation("feature X disabled", "feature X enabled"));
        assert!(!contains_negation("auth uses JWT", "db uses Postgres"));
    }

    #[test]
    fn competing_claims_detected() {
        let diff = competing_claim("deploy pakai Docker", "deploy pakai Kubernetes");
        assert!(diff.is_some(), "expected competing claim, got {diff:?}");
    }

    #[test]
    fn text_similarity_bounds() {
        assert_eq!(text_similarity("a b c", "a b c"), 1.0);
        assert_eq!(text_similarity("a b c", "x y z"), 0.0);
        assert!(text_similarity("", "") == 1.0);
    }

    #[test]
    fn contradiction_always_needs_review() {
        let a = ConflictAnalysis {
            relation: ConflictRelation::Contradicts,
            confidence: 0.99,
            reasoning: String::new(),
            key_difference: None,
            suggested_action: SuggestedAction::AskUser,
            existing_id: "x".into(),
            tier: 1,
        };
        let m = crate::test_support::sample_memory("a");
        assert!(should_auto_resolve(&a, &m, &m, &ConflictThresholds::default()).is_none());
    }
}
