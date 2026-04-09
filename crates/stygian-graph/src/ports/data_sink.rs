//! DataSink port — outbound counterpart to [`DataSourcePort`](super::data_source).
//!
//! [`DataSinkPort`] is the abstraction that lets pipeline nodes publish scraped
//! records to an external system without being coupled to any particular backend
//! (file system, webhook endpoint, message queue, Scrape Exchange, etc.).
//!
//! # Architecture
//!
//! Following the hexagonal architecture model:
//!
//! - This file lives in the **ports** layer — pure trait definitions, no I/O.
//! - Concrete adapters (file sink, HTTP sink, …) implement this trait and live
//!   under `adapters/`.
//!
//! # Example
//!
//! ```rust
//! use stygian_graph::ports::data_sink::{DataSinkPort, SinkRecord};
//!
//! // Any adapter that implements DataSinkPort can be used here.
//! async fn publish_one(sink: &dyn DataSinkPort, payload: serde_json::Value) {
//!     let record = SinkRecord::new("my-schema", "https://example.com", payload);
//!     match sink.publish(&record).await {
//!         Ok(receipt) => println!("Published: {}", receipt.id),
//!         Err(e) => eprintln!("Publish failed: {e}"),
//!     }
//! }
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that a [`DataSinkPort`] implementation may return.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DataSinkError {
    /// The record failed structural or semantic validation before being sent.
    #[error("validation failed: {0}")]
    ValidationFailed(String),

    /// The underlying transport or API rejected the publish request.
    #[error("publish failed: {0}")]
    PublishFailed(String),

    /// The sink is temporarily rate-limited; caller should back off.
    #[error("rate limited: {0}")]
    RateLimited(String),

    /// Authentication or authorisation rejected the request.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The referenced schema identifier is not known to this sink.
    #[error("schema not found: {0}")]
    SchemaNotFound(String),
}

// ── Domain types ──────────────────────────────────────────────────────────────

/// A single structured record to be published through a [`DataSinkPort`].
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::data_sink::SinkRecord;
/// use serde_json::json;
///
/// let record = SinkRecord::new(
///     "product-v1",
///     "https://shop.example.com/items/42",
///     json!({ "sku": "ABC-42", "price": 9.99 }),
/// );
/// assert_eq!(record.schema_id, "product-v1");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkRecord {
    /// The payload to publish. Any JSON value is accepted.
    pub data: serde_json::Value,

    /// Identifies the schema or data-contract version this record conforms to.
    /// Sinks may use this for routing, validation, or schema-registry lookups.
    pub schema_id: String,

    /// The canonical URL the record was scraped from. Used for provenance and
    /// deduplication. Stored as a `String` to avoid a `url` crate dependency
    /// in the port layer.
    pub source_url: String,

    /// Arbitrary string key-value metadata (content-type, run-id, tenant, …).
    pub metadata: HashMap<String, String>,
}

impl SinkRecord {
    /// Construct a new [`SinkRecord`] with empty metadata.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::ports::data_sink::SinkRecord;
    ///
    /// let r = SinkRecord::new("schema-v1", "https://example.com/page", serde_json::Value::Null);
    /// assert!(r.metadata.is_empty());
    /// ```
    pub fn new(
        schema_id: impl Into<String>,
        source_url: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            data,
            schema_id: schema_id.into(),
            source_url: source_url.into(),
            metadata: HashMap::new(),
        }
    }

    /// Attach a metadata entry and return `self` for builder-style use.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::ports::data_sink::SinkRecord;
    ///
    /// let r = SinkRecord::new("s", "https://x.com", serde_json::Value::Null)
    ///     .with_meta("run_id", "abc123");
    /// assert_eq!(r.metadata["run_id"], "abc123");
    /// ```
    #[must_use]
    pub fn with_meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// Confirmation that a [`SinkRecord`] was successfully accepted by the sink.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::data_sink::SinkReceipt;
///
/// let receipt = SinkReceipt {
///     id: "rec-001".to_string(),
///     published_at: "2026-04-09T00:00:00Z".to_string(),
///     platform: "file-sink".to_string(),
/// };
/// assert_eq!(receipt.platform, "file-sink");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkReceipt {
    /// Platform-assigned identifier for this published record.
    pub id: String,

    /// ISO 8601 timestamp at which the sink accepted the record.
    pub published_at: String,

    /// Human-readable name of the sink platform (e.g. `"scrape-exchange"`, `"file"`).
    pub platform: String,
}

// ── Port trait ────────────────────────────────────────────────────────────────

/// Outbound data sink port — publish scraped records to an external system.
///
/// Implementations live in `adapters/` and are never imported by domain code.
/// The port is always injected via `Arc<dyn DataSinkPort>`.
///
/// # Object safety
///
/// The trait uses `async fn` (Rust 2024 native async trait) and is therefore
/// object-safe with the `async_trait` erasure approach already used in this
/// workspace. Callers that need `dyn DataSinkPort` use it through `Arc`.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::data_sink::{DataSinkPort, SinkRecord, SinkReceipt, DataSinkError};
///
/// struct NoopSink;
///
/// #[async_trait::async_trait]
/// impl DataSinkPort for NoopSink {
///     async fn publish(&self, _record: &SinkRecord) -> Result<SinkReceipt, DataSinkError> {
///         Ok(SinkReceipt {
///             id: "noop".to_string(),
///             published_at: "".to_string(),
///             platform: "noop".to_string(),
///         })
///     }
///
///     async fn validate(&self, _record: &SinkRecord) -> Result<(), DataSinkError> {
///         Ok(())
///     }
///
///     async fn health_check(&self) -> Result<(), DataSinkError> {
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait DataSinkPort: Send + Sync {
    /// Validate and publish `record` to the sink.
    ///
    /// Implementations should validate the record before publishing; failing
    /// fast with [`DataSinkError::ValidationFailed`] is preferred over sending
    /// invalid data downstream.
    ///
    /// # Errors
    ///
    /// Returns [`DataSinkError`] on validation failure, transport error, or
    /// rate-limit/auth rejection.
    async fn publish(&self, record: &SinkRecord) -> Result<SinkReceipt, DataSinkError>;

    /// Validate `record` without publishing it.
    ///
    /// Useful for preflight checks without side effects.
    ///
    /// # Errors
    ///
    /// Returns [`DataSinkError::ValidationFailed`] if the record is malformed
    /// or violates schema constraints.
    async fn validate(&self, record: &SinkRecord) -> Result<(), DataSinkError>;

    /// Check that the sink backend is reachable and healthy.
    ///
    /// # Errors
    ///
    /// Returns [`DataSinkError::PublishFailed`] or [`DataSinkError::Unauthorized`]
    /// if the backend is unreachable or misconfigured.
    async fn health_check(&self) -> Result<(), DataSinkError>;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sink_record_construction_and_serde_roundtrip() {
        let record = SinkRecord::new(
            "product-v1",
            "https://shop.example.com/items/42",
            json!({ "sku": "ABC-42", "price": 9.99 }),
        )
        .with_meta("run_id", "abc123")
        .with_meta("tenant", "acme");

        assert_eq!(record.schema_id, "product-v1");
        assert_eq!(record.source_url, "https://shop.example.com/items/42");
        assert_eq!(record.data["sku"], "ABC-42");
        assert_eq!(record.metadata["run_id"], "abc123");
        assert_eq!(record.metadata["tenant"], "acme");

        // Round-trip through JSON
        let json_str = serde_json::to_string(&record).expect("serialize");
        let restored: SinkRecord = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(restored.schema_id, record.schema_id);
        assert_eq!(restored.source_url, record.source_url);
        assert_eq!(restored.metadata["run_id"], "abc123");
    }

    #[test]
    fn sink_receipt_serde_roundtrip() {
        let receipt = SinkReceipt {
            id: "rec-001".to_string(),
            published_at: "2026-04-09T00:00:00Z".to_string(),
            platform: "test-sink".to_string(),
        };

        let json_str = serde_json::to_string(&receipt).expect("serialize");
        let restored: SinkReceipt = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(restored.id, receipt.id);
        assert_eq!(restored.platform, receipt.platform);
    }

    #[test]
    fn data_sink_error_display() {
        assert_eq!(
            DataSinkError::ValidationFailed("missing field".to_string()).to_string(),
            "validation failed: missing field"
        );
        assert_eq!(
            DataSinkError::PublishFailed("timeout".to_string()).to_string(),
            "publish failed: timeout"
        );
        assert_eq!(
            DataSinkError::RateLimited("429".to_string()).to_string(),
            "rate limited: 429"
        );
        assert_eq!(
            DataSinkError::Unauthorized("401".to_string()).to_string(),
            "unauthorized: 401"
        );
        assert_eq!(
            DataSinkError::SchemaNotFound("v99".to_string()).to_string(),
            "schema not found: v99"
        );
    }
}
