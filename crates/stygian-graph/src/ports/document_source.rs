//! Document source port — read local files as pipeline data sources.
//!
//! Defines the [`DocumentSourcePort`] trait for reading documents (CSV, JSON,
//! Markdown, plain text, etc.) from the local file system and returning their
//! content for downstream processing.
//!
//! # Architecture
//!
//! ```text
//! stygian-graph
//!   ├─ DocumentSourcePort (this file)      ← always compiled
//!   └─ Adapters (adapters/)
//!        └─ DocumentSource                 → std::fs / tokio::fs
//! ```
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::ports::document_source::{DocumentSourcePort, DocumentQuery};
//! use std::path::PathBuf;
//!
//! async fn read_docs<D: DocumentSourcePort>(source: &D) {
//!     let query = DocumentQuery {
//!         path: PathBuf::from("data/input.csv"),
//!         recursive: false,
//!         glob_pattern: None,
//!     };
//!     let docs = source.read_documents(query).await.unwrap();
//!     for doc in &docs {
//!         println!("{}: {} bytes", doc.path.display(), doc.content.len());
//!     }
//! }
//! ```

use crate::domain::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// Document / DocumentQuery
// ─────────────────────────────────────────────────────────────────────────────

/// Query parameters for reading documents from the file system.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::document_source::DocumentQuery;
/// use std::path::PathBuf;
///
/// let query = DocumentQuery {
///     path: PathBuf::from("data/"),
///     recursive: true,
///     glob_pattern: Some("*.csv".into()),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentQuery {
    /// Path to a file or directory
    pub path: PathBuf,
    /// If `path` is a directory, whether to recurse into subdirectories
    pub recursive: bool,
    /// Optional glob pattern to filter files (e.g. `"*.json"`)
    pub glob_pattern: Option<String>,
}

/// A document read from the file system.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::document_source::Document;
/// use std::path::PathBuf;
///
/// let doc = Document {
///     path: PathBuf::from("data/input.csv"),
///     content: "id,name\n1,Alice\n".into(),
///     mime_type: Some("text/csv".into()),
///     size_bytes: 17,
/// };
/// assert_eq!(doc.size_bytes, 17);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Absolute or relative path to the file
    pub path: PathBuf,
    /// File content (text; binary files should be base64-encoded)
    pub content: String,
    /// Detected or inferred MIME type
    pub mime_type: Option<String>,
    /// File size in bytes
    pub size_bytes: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentSourcePort
// ─────────────────────────────────────────────────────────────────────────────

/// Port: read documents from the local file system.
///
/// Implementations handle file enumeration, glob filtering, and content
/// reading.  Binary files (PDFs, images) should be base64-encoded in the
/// `content` field and can be further processed by the multimodal adapter.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::ports::document_source::{DocumentSourcePort, DocumentQuery, Document};
/// use stygian_graph::domain::error::Result;
/// use async_trait::async_trait;
/// use std::path::PathBuf;
///
/// struct MockDocs;
///
/// #[async_trait]
/// impl DocumentSourcePort for MockDocs {
///     async fn read_documents(&self, query: DocumentQuery) -> Result<Vec<Document>> {
///         Ok(vec![Document {
///             path: query.path,
///             content: "hello".into(),
///             mime_type: Some("text/plain".into()),
///             size_bytes: 5,
///         }])
///     }
///
///     fn source_name(&self) -> &str {
///         "mock-docs"
///     }
/// }
/// ```
#[async_trait]
pub trait DocumentSourcePort: Send + Sync {
    /// Read documents matching the query.
    ///
    /// # Arguments
    ///
    /// * `query` - Path, recursion flag, and optional glob filter
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Document>)` - Matched documents with content
    /// * `Err(StygianError)` - I/O or permission error
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::document_source::{DocumentSourcePort, DocumentQuery};
    /// # use std::path::PathBuf;
    /// # async fn example(source: impl DocumentSourcePort) {
    /// let query = DocumentQuery {
    ///     path: PathBuf::from("data/report.json"),
    ///     recursive: false,
    ///     glob_pattern: None,
    /// };
    /// let docs = source.read_documents(query).await.unwrap();
    /// # }
    /// ```
    async fn read_documents(&self, query: DocumentQuery) -> Result<Vec<Document>>;

    /// Human-readable name of this document source.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::document_source::DocumentSourcePort;
    /// # fn example(source: impl DocumentSourcePort) {
    /// println!("Source: {}", source.source_name());
    /// # }
    /// ```
    fn source_name(&self) -> &str;
}
