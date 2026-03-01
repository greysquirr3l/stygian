//! Storage port — persist and retrieve pipeline results.
//!
//! Defines the generic [`StoragePort`] trait plus the [`OutputFormatter`] helper
//! that serialises pipeline outputs to CSV, JSONL, or JSON.
//!
//! # Architecture
//!
//! ```text
//! mycelium-graph
//!   ├─ StoragePort (this file)             ← always compiled
//!   └─ Adapters (adapters/)
//!        ├─ FileStorage       (always)     → writes .jsonl to disk
//!        ├─ NullStorage       (always)     → no-op for tests
//!        └─ PostgresStorage   (feature="postgres")  → sqlx PgPool
//! ```
//!
//! # Example — writing results
//!
//! ```no_run
//! use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
//! use serde_json::json;
//!
//! async fn persist<S: StoragePort>(storage: &S) {
//!     let record = StorageRecord::new("pipe-1", "fetch", json!({"url": "https://example.com"}));
//!     storage.store(record).await.unwrap();
//! }
//! ```

use crate::domain::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

// ─────────────────────────────────────────────────────────────────────────────
// StorageRecord
// ─────────────────────────────────────────────────────────────────────────────

/// A single result record produced by a pipeline node.
///
/// # Example
///
/// ```
/// use mycelium_graph::ports::storage::StorageRecord;
/// use serde_json::json;
///
/// let r = StorageRecord::new("pipe-1", "fetch", json!({"url": "https://example.com"}));
/// assert_eq!(r.pipeline_id, "pipe-1");
/// assert_eq!(r.node_name,   "fetch");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRecord {
    /// Unique record ID (UUID v4)
    pub id: String,
    /// Pipeline this record belongs to
    pub pipeline_id: String,
    /// Graph node that produced this record
    pub node_name: String,
    /// Extracted data payload
    pub data: Value,
    /// Optional key-value metadata (headers, status code, …)
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// Unix timestamp of when this record was created (milliseconds)
    pub timestamp_ms: u64,
}

impl StorageRecord {
    /// Construct a new record with a fresh UUID and current timestamp.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::ports::storage::StorageRecord;
    /// use serde_json::json;
    ///
    /// let r = StorageRecord::new("p", "n", json!(null));
    /// assert!(!r.id.is_empty());
    /// assert!(r.timestamp_ms > 0);
    /// ```
    pub fn new(pipeline_id: &str, node_name: &str, data: Value) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            id,
            pipeline_id: pipeline_id.to_string(),
            node_name: node_name.to_string(),
            data,
            metadata: Default::default(),
            timestamp_ms,
        }
    }

    /// Attach metadata key-value pairs.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::ports::storage::StorageRecord;
    /// use serde_json::json;
    ///
    /// let r = StorageRecord::new("p", "n", json!(null))
    ///     .with_metadata("status", "200");
    /// assert_eq!(r.metadata["status"], "200");
    /// ```
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StoragePort
// ─────────────────────────────────────────────────────────────────────────────

/// Port: persist and retrieve [`StorageRecord`]s produced by pipelines.
///
/// # Example
///
/// ```no_run
/// use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
/// use serde_json::json;
///
/// async fn run<S: StoragePort>(storage: &S) {
///     let r = StorageRecord::new("pipe-1", "fetch", json!({"url": "https://example.com"}));
///     storage.store(r.clone()).await.unwrap();
///
///     let fetched = storage.retrieve(&r.id).await.unwrap().unwrap();
///     assert_eq!(fetched.id, r.id);
/// }
/// ```
#[async_trait]
pub trait StoragePort: Send + Sync {
    /// Persist a record.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
    /// # use serde_json::json;
    /// # async fn example(s: impl StoragePort) {
    /// s.store(StorageRecord::new("p", "n", json!(null))).await.unwrap();
    /// # }
    /// ```
    async fn store(&self, record: StorageRecord) -> Result<()>;

    /// Retrieve a record by ID.  Returns `None` if not found.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
    /// # use serde_json::json;
    /// # async fn example(s: impl StoragePort) {
    /// let maybe = s.retrieve("some-id").await.unwrap();
    /// # }
    /// ```
    async fn retrieve(&self, id: &str) -> Result<Option<StorageRecord>>;

    /// List all records for a given `pipeline_id`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
    /// # use serde_json::json;
    /// # async fn example(s: impl StoragePort) {
    /// let records = s.list("pipe-1").await.unwrap();
    /// # }
    /// ```
    async fn list(&self, pipeline_id: &str) -> Result<Vec<StorageRecord>>;

    /// Delete a record by ID.  No-op if it does not exist.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
    /// # async fn example(s: impl StoragePort) {
    /// s.delete("some-id").await.unwrap();
    /// # }
    /// ```
    async fn delete(&self, id: &str) -> Result<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// OutputFormat + OutputFormatter
// ─────────────────────────────────────────────────────────────────────────────

/// Supported serialisation formats for pipeline result export.
///
/// # Example
///
/// ```
/// use mycelium_graph::ports::storage::OutputFormat;
///
/// let fmt = OutputFormat::Jsonl;
/// assert_eq!(fmt.extension(), "jsonl");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Newline-delimited JSON — one record per line
    #[default]
    Jsonl,
    /// CSV — header row + comma-separated values
    Csv,
    /// Pretty-printed JSON array
    Json,
}

impl OutputFormat {
    /// File extension for this format.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::ports::storage::OutputFormat;
    ///
    /// assert_eq!(OutputFormat::Csv.extension(), "csv");
    /// assert_eq!(OutputFormat::Json.extension(), "json");
    /// assert_eq!(OutputFormat::Jsonl.extension(), "jsonl");
    /// ```
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// Port: serialise a slice of [`StorageRecord`]s to bytes in a given format.
///
/// # Example
///
/// ```
/// use mycelium_graph::ports::storage::{OutputFormat, OutputFormatter, StorageRecord};
/// use mycelium_graph::domain::error::Result;
/// use serde_json::json;
///
/// struct JsonlFormatter;
///
/// impl OutputFormatter for JsonlFormatter {
///     fn format(&self, records: &[StorageRecord]) -> Result<Vec<u8>> {
///         let mut out = Vec::new();
///         for r in records {
///             let line = serde_json::to_string(r).unwrap();
///             out.extend_from_slice(line.as_bytes());
///             out.push(b'\n');
///         }
///         Ok(out)
///     }
///     fn format_type(&self) -> OutputFormat { OutputFormat::Jsonl }
/// }
/// ```
pub trait OutputFormatter: Send + Sync {
    /// Serialise `records` to owned bytes.
    fn format(&self, records: &[StorageRecord]) -> Result<Vec<u8>>;

    /// Which format this formatter produces.
    fn format_type(&self) -> OutputFormat;
}
