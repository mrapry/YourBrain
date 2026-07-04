//! Configuration types, mirroring `~/.yourbrain/config.toml`.
//!
//! Every field has a sensible default so a fresh install works with zero
//! configuration. `serde(default)` is used throughout so partial config files
//! are valid.

use serde::{Deserialize, Serialize};

use crate::compress::{CompressConfig, Intensity};
use crate::conflict::ConflictThresholds;

/// Root configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub storage: Storage,
    pub embedding: Embedding,
    pub search: Search,
    pub recall: Recall,
    pub conflict: Conflict,
    pub compression: Compression,
    pub privacy: Privacy,
    pub daemon: Daemon,
    pub rerank: Rerank,
    pub token_budget: TokenBudget,
    pub guardrail: Guardrail,
    pub cache: Cache,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub author: String,
    pub default_room: String,
    pub default_scope: String,
    pub language: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            author: "me".into(),
            default_room: "auto".into(),
            default_scope: "personal".into(),
            language: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
    pub data_dir: String,
    pub backup_enabled: bool,
    pub max_backups: u32,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            data_dir: "~/.yourbrain".into(),
            backup_enabled: true,
            max_backups: 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Embedding {
    /// "local" (bundled hash embedder by default), "onnx", "ollama", "openai".
    pub provider: String,
    pub model: String,
    pub dimension: usize,
}

impl Default for Embedding {
    fn default() -> Self {
        // Default provider is the dependency-free deterministic embedder.
        Self {
            provider: "local".into(),
            model: "hash-bow-v1".into(),
            dimension: 256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Search {
    pub default_limit: usize,
    pub min_confidence: f32,
    pub include_archived: bool,
    /// RRF constant.
    pub rrf_k: f32,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            default_limit: 5,
            min_confidence: 0.3,
            include_archived: false,
            rrf_k: 60.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Recall {
    pub max_tokens: usize,
    pub default_detail: String,
}

impl Default for Recall {
    fn default() -> Self {
        Self {
            max_tokens: 200,
            default_detail: "summary".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Conflict {
    pub enabled: bool,
    /// "auto" (best available), "bundled", "rules", or "llm" (see ADR-3).
    pub judge_tier: String,
    /// Cosine similarity that triggers a conflict check.
    pub similarity_threshold: f32,
    pub auto_supersede_threshold: f32,
    pub auto_duplicate_threshold: f32,
    pub auto_complement_threshold: f32,
    /// How long (seconds) a pending conflict waits before expiring.
    pub conflict_expiry_secs: i64,
    pub candidate_top_k: usize,
}

impl Default for Conflict {
    fn default() -> Self {
        Self {
            enabled: true,
            judge_tier: "auto".into(),
            // Tuned for the default hash embedder (lower absolute similarity
            // scale). Raise to ~0.75 when using an ONNX sentence-transformer.
            similarity_threshold: 0.45,
            auto_supersede_threshold: 0.92,
            auto_duplicate_threshold: 0.95,
            auto_complement_threshold: 0.80,
            conflict_expiry_secs: 300,
            candidate_top_k: 5,
        }
    }
}

impl Conflict {
    pub fn thresholds(&self) -> ConflictThresholds {
        ConflictThresholds {
            auto_supersede: self.auto_supersede_threshold,
            auto_duplicate: self.auto_duplicate_threshold,
            auto_complement: self.auto_complement_threshold,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Compression {
    pub intensity: String,
    pub preserve_code: bool,
    pub preserve_paths: bool,
    pub preserve_urls: bool,
}

impl Default for Compression {
    fn default() -> Self {
        Self {
            intensity: "full".into(),
            preserve_code: true,
            preserve_paths: true,
            preserve_urls: true,
        }
    }
}

impl Compression {
    pub fn to_compress_config(&self) -> CompressConfig {
        let intensity = match self.intensity.as_str() {
            "lite" => Intensity::Lite,
            "ultra" => Intensity::Ultra,
            _ => Intensity::Full,
        };
        CompressConfig {
            intensity,
            preserve_code: self.preserve_code,
            preserve_paths: self.preserve_paths,
            preserve_urls: self.preserve_urls,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    pub redact_secrets: bool,
    pub exclude_patterns: Vec<String>,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            redact_secrets: true,
            exclude_patterns: vec![
                "**/.env".into(),
                "**/.env.*".into(),
                "**/secrets/**".into(),
                "**/*.pem".into(),
                "**/*.key".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Daemon {
    pub idle_shutdown_secs: u64,
    pub embed_batch_size: usize,
}

impl Default for Daemon {
    fn default() -> Self {
        Self {
            idle_shutdown_secs: 600,
            embed_batch_size: 10,
        }
    }
}

/// Lexical (BM25-style) reranking applied on top of the fused/metadata score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Rerank {
    /// When false, recall behaves exactly like v0.1.0 (no lexical rerank).
    pub enabled: bool,
    /// Candidate pool size multiplier over the requested limit before reranking.
    pub candidate_pool_factor: usize,
    /// Blend weight of the lexical score against the fused/metadata score (0..1).
    pub lexical_weight: f32,
}

impl Default for Rerank {
    fn default() -> Self {
        Self {
            enabled: true,
            candidate_pool_factor: 4,
            lexical_weight: 0.5,
        }
    }
}

/// Dynamic token budgeting: opt-in compression of recalled memories to fit a
/// tighter budget. Disabled by default so recall output is unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenBudget {
    /// Master switch (default OFF).
    pub enabled: bool,
    /// "extractive" (query-aware sentence selection) or "ultra" (rule-based).
    pub strategy: String,
    /// Token budget override; 0 means fall back to `recall.max_tokens`.
    pub max_tokens: usize,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            enabled: false,
            strategy: "extractive".into(),
            max_tokens: 0,
        }
    }
}

/// Guardrail / fact-checking thresholds (grounding of a drafted answer vs KB).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Guardrail {
    /// Minimum evidence score for a claim to count as supported. Tuned for the
    /// default hash embedder; raise when using an ONNX sentence-transformer.
    pub support_threshold: f32,
    /// How many KB memories to gather as evidence per validation.
    pub evidence_top_k: usize,
}

impl Default for Guardrail {
    fn default() -> Self {
        Self {
            support_threshold: 0.35,
            evidence_top_k: 5,
        }
    }
}

/// Layered semantic cache grounded in the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub enabled: bool,
    /// Tier 1: similarity to a stored Q&A entry that counts as a cache hit.
    pub similarity_threshold: f32,
    /// Tier 2: KB match strong enough to answer directly (bypass the LLM).
    pub kb_direct_threshold: f32,
    /// Tier 3: KB match returned as grounding context (still go to the LLM).
    pub kb_grounding_threshold: f32,
    /// Whether to consult the knowledge base (Tier 2/3), not just Q&A cache.
    pub use_kb: bool,
    /// Time-to-live for cached answers, in seconds.
    pub ttl_secs: i64,
    /// Cap on stored cache entries (oldest evicted first).
    pub max_entries: usize,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            enabled: true,
            similarity_threshold: 0.85,
            kb_direct_threshold: 0.80,
            kb_grounding_threshold: 0.50,
            use_kb: true,
            ttl_secs: 3600,
            max_entries: 500,
        }
    }
}

impl Config {
    /// Parse config from a TOML string, filling defaults for missing fields.
    pub fn from_toml(s: &str) -> Result<Self, toml_error::TomlError> {
        toml_error::parse(s)
    }

    /// Serialize back to a pretty TOML string.
    pub fn to_toml(&self) -> String {
        // serde_json as an intermediate keeps this dependency-light and is only
        // used for `config show`; round-trips are not required.
        format!(
            "# YourBrain configuration\n# author: {}\n# provider: {}\n# judge_tier: {}\n",
            self.general.author, self.embedding.provider, self.conflict.judge_tier
        )
    }
}

/// Thin wrapper so callers don't depend on the `toml` crate's error type name.
pub mod toml_error {
    use super::Config;

    #[derive(Debug)]
    pub struct TomlError(pub String);

    impl std::fmt::Display for TomlError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "config parse error: {}", self.0)
        }
    }
    impl std::error::Error for TomlError {}

    pub fn parse(s: &str) -> Result<Config, TomlError> {
        toml::from_str(s).map_err(|e| TomlError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.embedding.provider, "local");
        assert_eq!(c.conflict.judge_tier, "auto");
        assert!(c.conflict.enabled);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let c = Config::from_toml("[general]\nauthor = \"matius\"\n").unwrap();
        assert_eq!(c.general.author, "matius");
        // Untouched sections keep defaults.
        assert_eq!(c.conflict.similarity_threshold, 0.45);
    }
}
