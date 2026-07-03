//! Core domain types: memories, edges, sessions, observations, and conflicts.
//!
//! These types are the canonical data model shared by every layer of the
//! engine (storage, ingestion, search, conflict resolution). They map directly
//! to the SQLite schema in [`crate::store`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::YbError;

/// Kind of knowledge a memory represents. Drives classification and ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Fact,
    Decision,
    Procedure,
    Solution,
    Preference,
    Event,
    /// Auto-captured from hooks; not user-authored knowledge.
    Observation,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Fact => "fact",
            MemoryType::Decision => "decision",
            MemoryType::Procedure => "procedure",
            MemoryType::Solution => "solution",
            MemoryType::Preference => "preference",
            MemoryType::Event => "event",
            MemoryType::Observation => "observation",
        }
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryType {
    type Err = YbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "fact" => MemoryType::Fact,
            "decision" => MemoryType::Decision,
            "procedure" => MemoryType::Procedure,
            "solution" => MemoryType::Solution,
            "preference" => MemoryType::Preference,
            "event" => MemoryType::Event,
            "observation" => MemoryType::Observation,
            other => {
                return Err(YbError::InvalidArgument(format!(
                    "unknown memory_type: {other}"
                )))
            }
        })
    }
}

/// Lifecycle state of a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Active,
    Disputed,
    Superseded,
    Archived,
    Pending,
}

impl MemoryState {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryState::Active => "active",
            MemoryState::Disputed => "disputed",
            MemoryState::Superseded => "superseded",
            MemoryState::Archived => "archived",
            MemoryState::Pending => "pending",
        }
    }
}

impl fmt::Display for MemoryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryState {
    type Err = YbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "active" => MemoryState::Active,
            "disputed" => MemoryState::Disputed,
            "superseded" => MemoryState::Superseded,
            "archived" => MemoryState::Archived,
            "pending" => MemoryState::Pending,
            other => return Err(YbError::InvalidArgument(format!("unknown state: {other}"))),
        })
    }
}

/// Visibility scope: personal (author-only) or team (shared).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Personal,
    Team,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Personal => "personal",
            Scope::Team => "team",
        }
    }
}

impl FromStr for Scope {
    type Err = YbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "personal" => Scope::Personal,
            "team" => Scope::Team,
            other => return Err(YbError::InvalidArgument(format!("unknown scope: {other}"))),
        })
    }
}

/// Where a memory originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Manual,
    Hook,
    Import,
}

impl SourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceType::Manual => "manual",
            SourceType::Hook => "hook",
            SourceType::Import => "import",
        }
    }
}

impl FromStr for SourceType {
    type Err = YbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "manual" => SourceType::Manual,
            "hook" => SourceType::Hook,
            "import" => SourceType::Import,
            other => return Err(YbError::InvalidArgument(format!("unknown source: {other}"))),
        })
    }
}

/// Relationship type between two memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Supersedes,
    Contradicts,
    Complements,
    References,
    SimilarTo,
    DerivedFrom,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeType::Supersedes => "supersedes",
            EdgeType::Contradicts => "contradicts",
            EdgeType::Complements => "complements",
            EdgeType::References => "references",
            EdgeType::SimilarTo => "similar_to",
            EdgeType::DerivedFrom => "derived_from",
        }
    }
}

impl FromStr for EdgeType {
    type Err = YbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "supersedes" => EdgeType::Supersedes,
            "contradicts" => EdgeType::Contradicts,
            "complements" => EdgeType::Complements,
            "references" => EdgeType::References,
            "similar_to" => EdgeType::SimilarTo,
            "derived_from" => EdgeType::DerivedFrom,
            other => {
                return Err(YbError::InvalidArgument(format!(
                    "unknown edge_type: {other}"
                )))
            }
        })
    }
}

/// A directed relationship between two memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub edge_type: EdgeType,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// The core unit of stored knowledge.
///
/// Three text representations are kept (see ADR-11):
/// - `content`: verbatim original, never modified.
/// - `compressed`: rule-based abbreviation, near-lossless (~30% smaller).
/// - `summary` / `headline`: lossy retrieval aids for token-efficient recall.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub compressed: String,
    pub summary: String,
    pub headline: String,

    pub memory_type: MemoryType,
    pub state: MemoryState,
    pub scope: Scope,

    pub author: String,
    pub room: Option<String>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub source_type: SourceType,
    #[serde(default)]
    pub source_detail: Option<String>,

    pub confidence: f32,
    pub importance: f32,
    pub access_count: u32,
    pub last_accessed: Option<DateTime<Utc>>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,

    #[serde(default)]
    pub endorsed_by: Vec<String>,
    #[serde(default)]
    pub disputed_by: Vec<String>,
}

impl Memory {
    /// Compute a team-consensus confidence using Laplace smoothing over the
    /// endorsement/dispute counts. Independent of temporal decay.
    pub fn consensus_confidence(&self) -> f32 {
        let e = self.endorsed_by.len() as f32;
        let d = self.disputed_by.len() as f32;
        (e + 1.0) / (e + d + 2.0)
    }
}

/// An IDE session captured via hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub ide: String,
    pub cwd: Option<String>,
    pub room: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub metadata: Option<String>,
}

/// A raw hook capture (prompt, tool use, response, error).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub content: String,
    pub compressed: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: Option<String>,
}
