//! File system document source adapter.
//!
//! Implements [`DocumentSourcePort`] and [`ScrapingService`] for reading files
//! from the local file system.  Supports glob-based file discovery, recursive
//! directory traversal, and MIME-type detection.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::document::DocumentSource;
//! use stygian_graph::ports::document_source::{DocumentSourcePort, DocumentQuery};
//! use std::path::PathBuf;
//!
//! # async fn example() {
//! let source = DocumentSource::new();
//! let query = DocumentQuery {
//!     path: PathBuf::from("data/"),
//!     recursive: true,
//!     glob_pattern: Some("*.json".into()),
//! };
//! let docs = source.read_documents(query).await.unwrap();
//! # }
//! ```

use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::document_source::{Document, DocumentQuery, DocumentSourcePort};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─────────────────────────────────────────────────────────────────────────────
// DocumentSource
// ─────────────────────────────────────────────────────────────────────────────

/// Adapter: local file system as a pipeline data source.
///
/// Reads files from disk and returns their content as [`Document`] structs.
/// Also implements [`ScrapingService`] for DAG pipeline integration.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::document::DocumentSource;
///
/// let source = DocumentSource::new();
/// assert_eq!(source.source_name(), "filesystem");
/// ```
pub struct DocumentSource {
    _priv: (),
}

impl DocumentSource {
    /// Create a new file system document source.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::document::DocumentSource;
    ///
    /// let source = DocumentSource::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self { _priv: () }
    }

    /// Return the name of this source.
    #[must_use]
    pub fn source_name(&self) -> &str {
        "filesystem"
    }

    /// Collect file paths matching the query.
    async fn collect_paths(query: &DocumentQuery) -> Result<Vec<PathBuf>> {
        let meta = fs::metadata(&query.path).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "cannot access {}: {e}",
                query.path.display()
            )))
        })?;

        if meta.is_file() {
            return Ok(vec![query.path.clone()]);
        }

        let mut paths = Vec::new();
        Self::walk_dir(
            &query.path,
            query.recursive,
            &query.glob_pattern,
            &mut paths,
        )
        .await?;
        Ok(paths)
    }

    /// Recursively walk a directory collecting matching files.
    async fn walk_dir(
        dir: &Path,
        recursive: bool,
        glob: &Option<String>,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        let mut entries = fs::read_dir(dir).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "cannot read directory {}: {e}",
                dir.display()
            )))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!("readdir error: {e}")))
        })? {
            let path = entry.path();
            let ft = entry.file_type().await.map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!("file_type error: {e}")))
            })?;

            if ft.is_symlink() {
                // Follow symlinks: re-check metadata to determine the target type.
                match fs::metadata(&path).await {
                    Ok(meta) if meta.is_dir() && recursive => {
                        Box::pin(Self::walk_dir(&path, recursive, glob, out)).await?;
                    }
                    Ok(meta) if meta.is_file() => {
                        if let Some(pattern) = glob {
                            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                                && Self::glob_matches(pattern, name)
                            {
                                out.push(path);
                            }
                        } else {
                            out.push(path);
                        }
                    }
                    // Broken symlink or permission error — skip silently.
                    _ => {}
                }
            } else if ft.is_dir() && recursive {
                Box::pin(Self::walk_dir(&path, recursive, glob, out)).await?;
            } else if ft.is_file() {
                if let Some(pattern) = glob {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str())
                        && Self::glob_matches(pattern, name)
                    {
                        out.push(path);
                    }
                } else {
                    out.push(path);
                }
            }
        }

        Ok(())
    }

    /// Simple glob matching supporting `*` wildcard.
    ///
    /// Matching is case-insensitive to behave consistently across
    /// Windows (NTFS), macOS (APFS/HFS+), and Linux (ext4).
    fn glob_matches(pattern: &str, name: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        let pattern_lower = pattern.to_ascii_lowercase();
        let name_lower = name.to_ascii_lowercase();
        if let Some(ext) = pattern_lower.strip_prefix("*.") {
            return name_lower.ends_with(&format!(".{ext}"));
        }
        if let Some(prefix) = pattern_lower.strip_suffix('*') {
            return name_lower.starts_with(prefix);
        }
        pattern_lower == name_lower
    }

    /// Infer MIME type from file extension.
    ///
    /// Extension matching is case-insensitive so `.JSON`, `.Json`, etc.
    /// resolve correctly on all platforms.
    fn infer_mime(path: &Path) -> Option<String> {
        let ext_raw = path.extension()?.to_str()?;
        let ext = ext_raw.to_ascii_lowercase();
        let mime = match ext.as_str() {
            "json" => "application/json",
            "csv" => "text/csv",
            "xml" => "application/xml",
            "html" | "htm" => "text/html",
            "md" | "markdown" => "text/markdown",
            "txt" => "text/plain",
            "yaml" | "yml" => "application/yaml",
            "toml" => "application/toml",
            "pdf" => "application/pdf",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            _ => return None,
        };
        Some(mime.to_string())
    }
}

impl Default for DocumentSource {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentSourcePort
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl DocumentSourcePort for DocumentSource {
    async fn read_documents(&self, query: DocumentQuery) -> Result<Vec<Document>> {
        let paths = Self::collect_paths(&query).await?;
        let mut docs = Vec::with_capacity(paths.len());

        for path in paths {
            let content = fs::read_to_string(&path).await.map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "cannot read {}: {e}",
                    path.display()
                )))
            })?;
            let size_bytes = content.len() as u64;
            let mime_type = Self::infer_mime(&path);

            docs.push(Document {
                path,
                content,
                mime_type,
                size_bytes,
            });
        }

        Ok(docs)
    }

    fn source_name(&self) -> &str {
        "filesystem"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScrapingService (DAG integration)
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for DocumentSource {
    /// Read files from the local file system.
    ///
    /// Expected params:
    /// ```json
    /// { "path": "data/", "recursive": true, "glob_pattern": "*.csv" }
    /// ```
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// # use stygian_graph::adapters::document::DocumentSource;
    /// # use serde_json::json;
    /// # async fn example() {
    /// let source = DocumentSource::new();
    /// let input = ServiceInput {
    ///     url: String::new(),
    ///     params: json!({"path": "data/report.json"}),
    /// };
    /// let result = source.execute(input).await.unwrap();
    /// # }
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let path_str = input.params["path"].as_str().ok_or_else(|| {
            StygianError::Service(ServiceError::InvalidResponse(
                "missing 'path' in params".into(),
            ))
        })?;

        let query = DocumentQuery {
            path: PathBuf::from(path_str),
            recursive: input.params["recursive"].as_bool().unwrap_or(false),
            glob_pattern: input.params["glob_pattern"].as_str().map(String::from),
        };

        let docs = self.read_documents(query).await?;
        let doc_count = docs.len();

        Ok(ServiceOutput {
            data: serde_json::to_string(&docs).unwrap_or_default(),
            metadata: json!({
                "source": "filesystem",
                "document_count": doc_count,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "document"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_exact() {
        assert!(DocumentSource::glob_matches("report.csv", "report.csv"));
        assert!(!DocumentSource::glob_matches("report.csv", "other.csv"));
    }

    #[test]
    fn glob_matches_extension() {
        assert!(DocumentSource::glob_matches("*.csv", "data.csv"));
        assert!(!DocumentSource::glob_matches("*.csv", "data.json"));
    }

    #[test]
    fn glob_matches_case_insensitive() {
        assert!(DocumentSource::glob_matches("*.JSON", "data.json"));
        assert!(DocumentSource::glob_matches("*.csv", "DATA.CSV"));
        assert!(DocumentSource::glob_matches("Report*", "report_2024.csv"));
    }

    #[test]
    fn glob_matches_prefix() {
        assert!(DocumentSource::glob_matches("report*", "report_2024.csv"));
        assert!(!DocumentSource::glob_matches("report*", "data.csv"));
    }

    #[test]
    fn glob_matches_star() {
        assert!(DocumentSource::glob_matches("*", "anything.txt"));
    }

    #[test]
    fn infer_mime_known_types() {
        assert_eq!(
            DocumentSource::infer_mime(Path::new("data.json")),
            Some("application/json".into())
        );
        assert_eq!(
            DocumentSource::infer_mime(Path::new("data.csv")),
            Some("text/csv".into())
        );
        assert_eq!(
            DocumentSource::infer_mime(Path::new("doc.pdf")),
            Some("application/pdf".into())
        );
    }

    #[test]
    fn infer_mime_case_insensitive() {
        assert_eq!(
            DocumentSource::infer_mime(Path::new("DATA.JSON")),
            Some("application/json".into())
        );
        assert_eq!(
            DocumentSource::infer_mime(Path::new("photo.JPG")),
            Some("image/jpeg".into())
        );
    }

    #[test]
    fn infer_mime_unknown() {
        assert_eq!(DocumentSource::infer_mime(Path::new("data.xyz")), None);
    }

    #[tokio::test]
    async fn read_nonexistent_path_returns_error() {
        let source = DocumentSource::new();
        let query = DocumentQuery {
            path: PathBuf::from("/nonexistent/path/that/does/not/exist"),
            recursive: false,
            glob_pattern: None,
        };
        let result = source.read_documents(query).await;
        assert!(result.is_err());
    }
}
