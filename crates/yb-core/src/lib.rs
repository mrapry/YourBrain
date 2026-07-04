//! # yb-core
//!
//! Core library for **YourBrain** — an AI memory engine with conflict
//! resolution. This crate is UI-agnostic: the CLI, MCP server, hook handler,
//! and daemon all build on the [`brain::Brain`] facade.
//!
//! ## Module map
//! - [`memory`] — domain types (Memory, Edge, Session, …).
//! - [`store`] — SQLite persistence (schema, FTS5, embed queue, conflicts, cache).
//! - [`embed`] — embedding trait + dependency-free hash backend.
//! - [`vector`] — vector index trait + flat cosine backend.
//! - [`compress`] — rule-based compression into three levels.
//! - [`classify`] — privacy preprocessing + rule-based classification.
//! - [`search`] — RRF fusion, re-ranking, token-budgeted recall.
//! - [`rerank`] — lexical (BM25) reranking trait + default backend.
//! - [`budget`] — dynamic token budgeting / extractive summarization.
//! - [`guardrail`] — answer fact-checking against the knowledge base.
//! - [`conflict`] — tiered conflict detection and resolution.
//! - [`config`] — configuration types.
//! - [`brain`] — the high-level engine tying it all together.
//!
//! ## Design note
//! Heavy native backends (ONNX embeddings, HNSW/usearch) sit behind the
//! [`embed::Embedder`] and [`vector::VectorIndex`] traits. The default build
//! uses pure-Rust implementations so the whole system compiles, tests, and runs
//! anywhere without a C++/ONNX toolchain.

pub mod brain;
pub mod budget;
pub mod classify;
pub mod compress;
pub mod config;
pub mod conflict;
pub mod embed;
pub mod error;
pub mod guardrail;
pub mod memory;
pub mod rerank;
pub mod search;
pub mod store;
pub mod vector;

pub use brain::{Brain, RememberOptions, RememberOutcome, ResolveOutcome};
pub use config::Config;
pub use error::{Result, YbError};

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared fixtures for unit tests.
    use crate::memory::{Memory, MemoryState, MemoryType, Scope, SourceType};
    use chrono::Utc;

    /// Build a minimal, valid [`Memory`] with the given content.
    pub fn sample_memory(content: &str) -> Memory {
        let now = Utc::now();
        Memory {
            id: ulid::Ulid::new().to_string(),
            content: content.to_string(),
            compressed: content.to_string(),
            summary: content.to_string(),
            headline: content.chars().take(24).collect(),
            memory_type: MemoryType::Fact,
            state: MemoryState::Active,
            scope: Scope::Personal,
            author: "tester".to_string(),
            room: None,
            tags: vec![],
            entities: vec![],
            source_type: SourceType::Manual,
            source_detail: None,
            confidence: 0.8,
            importance: 0.5,
            access_count: 0,
            last_accessed: None,
            created_at: now,
            updated_at: now,
            verified_at: None,
            endorsed_by: vec![],
            disputed_by: vec![],
        }
    }
}
