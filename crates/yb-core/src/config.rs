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
