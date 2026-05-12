//! Error types for stygian-plugin

use thiserror::Error;

/// Result type for stygian-plugin operations
pub type Result<T> = std::result::Result<T, PluginError>;

/// Errors that can occur during plugin operations
#[derive(Debug, Error)]
pub enum PluginError {
    /// Template not found by ID
    #[error("template not found: {0}")]
    TemplateNotFound(String),

    /// Template validation failed
    #[error("template validation failed: {0}")]
    TemplateValidationError(String),

    /// Selector evaluation failed
    #[error("selector evaluation failed: {selector}, reason: {reason}")]
    SelectorError { selector: String, reason: String },

    /// Extraction failed
    #[error("extraction failed: {0}")]
    ExtractionError(String),

    /// Idempotency key already processed
    #[error("extraction already completed with idempotency key: {0}")]
    IdempotencyDuplicate(String),

    /// Idempotency store error
    #[error("idempotency store error: {0}")]
    IdempotencyStoreError(String),

    /// Template storage error
    #[error("template storage error: {0}")]
    StorageError(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// IO error
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),

    /// Timeout occurred
    #[error("timeout occurred")]
    Timeout,

    /// Invalid transformation configuration
    #[error("invalid transformation: {0}")]
    InvalidTransformation(String),

    /// Schema validation failed
    #[error("schema validation failed: {0}")]
    SchemaValidationError(String),

    /// Generic plugin error
    #[error("{0}")]
    Other(String),
}
