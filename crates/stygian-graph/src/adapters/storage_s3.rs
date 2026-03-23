//! S3-compatible object storage adapter.
//!
//! Implements [`StoragePort`] for AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces,
//! and any S3-compatible endpoint.
//!
//! Also implements [`ScrapingService`] so objects stored in S3 can be used as pipeline
//! input sources.
//!
//! # Feature gate
//!
//! Requires `feature = "object-storage"`.
//!
//! # Key structure
//!
//! Objects are stored under `{prefix}/{pipeline_id}/{node_name}/{record_id}.json`
//! for easy browsing and lifecycle rules.
//!
//! # Authentication
//!
//! Reads `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` from the environment (or
//! from the [`S3StorageConfig`]).  For non-AWS providers set `endpoint` to your
//! custom S3-compatible URL (MinIO, R2, Spaces, Backblaze B2, etc.).

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::storage::{StoragePort, StorageRecord};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
use async_trait::async_trait;
use s3::creds::Credentials;
use s3::{Bucket, Region};
use serde_json::json;

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for the S3 storage adapter.
#[derive(Debug, Clone)]
pub struct S3StorageConfig {
    /// S3 bucket name.
    pub bucket: String,
    /// AWS region name (e.g. `us-east-1`).  Ignored when `endpoint` is set.
    pub region: String,
    /// Optional custom endpoint for non-AWS providers.
    pub endpoint: Option<String>,
    /// Key prefix prepended to all object keys (default: `"stygian"`).
    pub prefix: String,
    /// Path-style access (required by MinIO and some providers).
    pub path_style: bool,
    /// Optional access key (falls back to env `AWS_ACCESS_KEY_ID`).
    pub access_key: Option<String>,
    /// Optional secret key (falls back to env `AWS_SECRET_ACCESS_KEY`).
    pub secret_key: Option<String>,
}

impl Default for S3StorageConfig {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            region: "us-east-1".to_string(),
            endpoint: None,
            prefix: "stygian".to_string(),
            path_style: false,
            access_key: None,
            secret_key: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// S3Storage adapter
// ─────────────────────────────────────────────────────────────────────────────

/// S3-compatible object storage adapter implementing [`StoragePort`].
pub struct S3Storage {
    bucket: Box<Bucket>,
    prefix: String,
}

/// Multipart upload threshold (5 MiB).
const MULTIPART_THRESHOLD: usize = 5 * 1024 * 1024;

impl S3Storage {
    /// Create a new [`S3Storage`] from the given config.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Service`] if credentials cannot be resolved or
    /// the bucket cannot be initialised.
    pub fn new(config: S3StorageConfig) -> Result<Self> {
        let credentials = match (&config.access_key, &config.secret_key) {
            (Some(ak), Some(sk)) => Credentials::new(Some(ak), Some(sk), None, None, None)
                .map_err(|e| {
                    StygianError::Service(ServiceError::AuthenticationFailed(format!(
                        "S3 credentials error: {e}"
                    )))
                })?,
            _ => Credentials::from_env().map_err(|e| {
                StygianError::Service(ServiceError::AuthenticationFailed(format!(
                    "S3 credentials from env: {e}"
                )))
            })?,
        };

        let region = match &config.endpoint {
            Some(endpoint) => Region::Custom {
                region: config.region.clone(),
                endpoint: endpoint.clone(),
            },
            None => config.region.parse::<Region>().map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "invalid S3 region '{}': {e}",
                    config.region
                )))
            })?,
        };

        let mut bucket = Bucket::new(&config.bucket, region, credentials).map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "S3 bucket init failed: {e}"
            )))
        })?;

        if config.path_style {
            bucket.set_path_style();
        }

        Ok(Self {
            bucket,
            prefix: config.prefix,
        })
    }

    /// Build the object key for a [`StorageRecord`].
    fn record_key(&self, record: &StorageRecord) -> String {
        let safe_pipeline = sanitise(&record.pipeline_id);
        let safe_node = sanitise(&record.node_name);
        format!(
            "{}/{}/{}/{}.json",
            self.prefix, safe_pipeline, safe_node, record.id
        )
    }

    /// Build the object key from a record id.
    ///
    /// Because `retrieve` and `delete` only receive an id, we store a metadata
    /// index object at `{prefix}/_index/{id}` that contains the full key.
    fn index_key(&self, id: &str) -> String {
        format!("{}/_index/{}", self.prefix, sanitise(id))
    }

    /// List prefix for all objects belonging to a pipeline.
    fn pipeline_prefix(&self, pipeline_id: &str) -> String {
        format!("{}/{}/", self.prefix, sanitise(pipeline_id))
    }

    /// Put an object, using multipart upload when the payload exceeds [`MULTIPART_THRESHOLD`].
    async fn put_object(&self, key: &str, body: &[u8], content_type: &str) -> Result<()> {
        if body.len() > MULTIPART_THRESHOLD {
            self.bucket
                .put_object_stream_with_content_type(
                    &mut std::io::Cursor::new(body),
                    key,
                    content_type,
                )
                .await
                .map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "S3 multipart PUT '{key}' failed: {e}"
                    )))
                })?;
        } else {
            self.bucket
                .put_object_with_content_type(key, body, content_type)
                .await
                .map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "S3 PUT '{key}' failed: {e}"
                    )))
                })?;
        }
        Ok(())
    }
}

/// Replace path-unsafe characters.
fn sanitise(s: &str) -> String {
    s.replace(['/', '\\', '.', ':', ' '], "_")
}

// ─────────────────────────────────────────────────────────────────────────────
// StoragePort
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl StoragePort for S3Storage {
    async fn store(&self, record: StorageRecord) -> Result<()> {
        let key = self.record_key(&record);

        let body = serde_json::to_vec(&record).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "S3 serialise record failed: {e}"
            )))
        })?;

        // Store the object
        self.put_object(&key, &body, "application/json").await?;

        // Store an index entry so retrieve/delete can locate the object by id
        let idx_key = self.index_key(&record.id);
        self.put_object(idx_key.as_str(), key.as_bytes(), "text/plain")
            .await?;

        Ok(())
    }

    async fn retrieve(&self, id: &str) -> Result<Option<StorageRecord>> {
        // Look up the full key via the index
        let idx_key = self.index_key(id);
        let idx_resp = self.bucket.get_object(&idx_key).await;

        let full_key = match idx_resp {
            Ok(resp) if resp.status_code() == 200 => {
                String::from_utf8_lossy(resp.as_slice()).to_string()
            }
            Ok(_) | Err(_) => return Ok(None),
        };

        let resp = self.bucket.get_object(&full_key).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "S3 GET '{full_key}' failed: {e}"
            )))
        })?;

        if resp.status_code() != 200 {
            return Ok(None);
        }

        let record: StorageRecord = serde_json::from_slice(resp.as_slice()).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "S3 deserialise record failed: {e}"
            )))
        })?;

        Ok(Some(record))
    }

    async fn list(&self, pipeline_id: &str) -> Result<Vec<StorageRecord>> {
        let prefix = self.pipeline_prefix(pipeline_id);

        let results = self.bucket.list(prefix.clone(), None).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "S3 LIST prefix '{prefix}' failed: {e}"
            )))
        })?;

        let mut records = Vec::new();
        for list_result in &results {
            for obj in &list_result.contents {
                // Skip index entries
                if obj.key.contains("/_index/") {
                    continue;
                }
                let resp = self.bucket.get_object(&obj.key).await.map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "S3 GET '{}' failed: {e}",
                        obj.key
                    )))
                })?;
                if resp.status_code() == 200
                    && let Ok(record) = serde_json::from_slice::<StorageRecord>(resp.as_slice())
                {
                    records.push(record);
                }
            }
        }

        Ok(records)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        // Look up the full key via the index
        let idx_key = self.index_key(id);
        let idx_resp = self.bucket.get_object(&idx_key).await;

        if let Ok(resp) = idx_resp
            && resp.status_code() == 200
        {
            let full_key = String::from_utf8_lossy(resp.as_slice()).to_string();

            // Delete the object
            let _ = self.bucket.delete_object(&full_key).await;

            // Delete the index entry
            let _ = self.bucket.delete_object(&idx_key).await;
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScrapingService — fetch objects from S3 as pipeline input
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for S3Storage {
    /// Fetch an object from S3 by key.
    ///
    /// The `input.url` is treated as the S3 object key (or prefix for listing).
    /// Params:
    /// - `"action"`: `"get"` (default) or `"list"`
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let action = input
            .params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("get");

        match action {
            "list" => {
                let prefix = input.url;
                let results = self.bucket.list(prefix.clone(), None).await.map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "S3 LIST '{prefix}' failed: {e}"
                    )))
                })?;

                let keys: Vec<&str> = results
                    .iter()
                    .flat_map(|r| r.contents.iter().map(|o| o.key.as_str()))
                    .collect();

                Ok(ServiceOutput {
                    data: serde_json::to_string(&keys).unwrap_or_default(),
                    metadata: json!({
                        "source": "s3",
                        "action": "list",
                        "prefix": prefix,
                        "count": keys.len(),
                    }),
                })
            }
            _ => {
                // "get" — fetch a single object
                let key = &input.url;
                let resp = self.bucket.get_object(key).await.map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "S3 GET '{key}' failed: {e}"
                    )))
                })?;

                if resp.status_code() != 200 {
                    return Err(StygianError::Service(ServiceError::InvalidResponse(
                        format!("S3 GET '{key}' returned status {}", resp.status_code()),
                    )));
                }

                let data = String::from_utf8_lossy(resp.as_slice()).to_string();

                Ok(ServiceOutput {
                    data,
                    metadata: json!({
                        "source": "s3",
                        "action": "get",
                        "key": key,
                        "size": resp.as_slice().len(),
                    }),
                })
            }
        }
    }

    fn name(&self) -> &'static str {
        "s3-storage"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitise() {
        assert_eq!(sanitise("pipe/1"), "pipe_1");
        assert_eq!(sanitise("a.b:c\\d e"), "a_b_c_d_e");
    }

    #[test]
    fn test_record_key_structure() {
        // We can't create a real S3Storage without creds, so test sanitise + format directly
        let prefix = "stygian";
        let pipeline_id = "my-pipeline";
        let node_name = "fetch";
        let id = "abc-123";
        let key = format!(
            "{}/{}/{}/{}.json",
            prefix,
            sanitise(pipeline_id),
            sanitise(node_name),
            id
        );
        assert_eq!(key, "stygian/my-pipeline/fetch/abc-123.json");
    }

    #[test]
    fn test_index_key_structure() {
        let prefix = "stygian";
        let id = "abc-123";
        let key = format!("{}/_index/{}", prefix, sanitise(id));
        assert_eq!(key, "stygian/_index/abc-123");
    }

    #[test]
    fn test_pipeline_prefix_structure() {
        let prefix = "stygian";
        let pipeline_id = "pipe/1";
        let pfx = format!("{}/{}/", prefix, sanitise(pipeline_id));
        assert_eq!(pfx, "stygian/pipe_1/");
    }

    #[test]
    fn test_default_config() {
        let cfg = S3StorageConfig::default();
        assert_eq!(cfg.region, "us-east-1");
        assert_eq!(cfg.prefix, "stygian");
        assert!(!cfg.path_style);
        assert!(cfg.endpoint.is_none());
        assert!(cfg.access_key.is_none());
        assert!(cfg.secret_key.is_none());
    }

    #[test]
    fn test_sanitise_preserves_safe_chars() {
        assert_eq!(sanitise("hello-world_123"), "hello-world_123");
    }

    #[test]
    fn test_key_with_special_pipeline_id() {
        let prefix = "data";
        let pipeline_id = "org/team:project.v2";
        let node_name = "extract";
        let id = "uuid-1";
        let key = format!(
            "{}/{}/{}/{}.json",
            prefix,
            sanitise(pipeline_id),
            sanitise(node_name),
            id
        );
        assert_eq!(key, "data/org_team_project_v2/extract/uuid-1.json");
    }
}
