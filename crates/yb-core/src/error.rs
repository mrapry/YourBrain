//! Error types for the YourBrain core library.

use thiserror::Error;

/// Result alias used across the core library.
pub type Result<T> = std::result::Result<T, YbError>;

/// The unified error type for all core operations.
#[derive(Debug, Error)]
pub enum YbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("memory not found: {0}")]
    NotFound(String),

    #[error("conflict not found: {0}")]
    ConflictNotFound(String),

    /// The configured embedding model does not match the one the database was
    /// created with. Changing models requires an explicit re-index because the
    /// vector dimension is locked at creation time (see ADR-5).
    #[error("embedding model mismatch: configured `{configured}`, stored `{stored}`. {hint}")]
    ModelMismatch {
        configured: String,
        stored: String,
        hint: String,
    },

    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Failure while initializing or running an embedding backend (e.g. the
    /// optional ONNX model failed to download or load).
    #[error("embedder error: {0}")]
    Embedder(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl YbError {
    /// Convenience constructor for ad-hoc error messages.
    pub fn other(msg: impl Into<String>) -> Self {
        YbError::Other(msg.into())
    }
}
