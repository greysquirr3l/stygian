//! Storage adapters — persist and retrieve pipeline [`StorageRecord`](crate::ports::storage::StorageRecord)s.
//!
//! # Adapters
//!
//! | Adapter | Availability | Backing store |
//! |---------|--------------|---------------|
//! | [`NullStorage`](storage::NullStorage) | always | no-op (tests / dry-run) |
//! | [`FileStorage`](storage::FileStorage) | always | `.jsonl` files on local disk |
//! | [`PostgresStorage`](storage::PostgresStorage) | `feature = "postgres"` | PostgreSQL via sqlx |

use crate::domain::error::{MyceliumError, Result, ServiceError};
use crate::ports::storage::{StoragePort, StorageRecord};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

// ─────────────────────────────────────────────────────────────────────────────
// NullStorage
// ─────────────────────────────────────────────────────────────────────────────

/// No-op storage adapter — discards all records.
///
/// Useful for dry-run mode and unit tests where persistence is not required.
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::storage::NullStorage;
/// use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
/// use serde_json::json;
///
/// # tokio_test::block_on(async {
/// let s = NullStorage;
/// s.store(StorageRecord::new("p", "n", json!(null))).await.unwrap();
/// let result = s.retrieve("any-id").await.unwrap();
/// assert!(result.is_none());
/// # });
/// ```
pub struct NullStorage;

#[async_trait]
impl StoragePort for NullStorage {
    async fn store(&self, _record: StorageRecord) -> Result<()> {
        Ok(())
    }

    async fn retrieve(&self, _id: &str) -> Result<Option<StorageRecord>> {
        Ok(None)
    }

    async fn list(&self, _pipeline_id: &str) -> Result<Vec<StorageRecord>> {
        Ok(vec![])
    }

    async fn delete(&self, _id: &str) -> Result<()> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FileStorage
// ─────────────────────────────────────────────────────────────────────────────

/// File-based storage adapter — one `.jsonl` file per pipeline.
///
/// Each pipeline gets its own file: `<dir>/<pipeline_id>.jsonl`.
/// Records are appended one JSON object per line.
///
/// # Example
///
/// ```no_run
/// use mycelium_graph::adapters::storage::FileStorage;
/// use mycelium_graph::ports::storage::{StoragePort, StorageRecord};
/// use serde_json::json;
/// use std::path::PathBuf;
///
/// # tokio_test::block_on(async {
/// let storage = FileStorage::new(PathBuf::from("/tmp/mycelium-results"));
/// let r = StorageRecord::new("pipe-1", "fetch", json!({"url": "https://example.com"}));
/// storage.store(r).await.unwrap();
/// # });
/// ```
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    /// Create a [`FileStorage`] backed by `dir`.
    ///
    /// The directory will be created on first write if it does not exist.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::adapters::storage::FileStorage;
    /// use std::path::PathBuf;
    ///
    /// let s = FileStorage::new(PathBuf::from("/tmp/data"));
    /// ```
    pub const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn pipeline_file(&self, pipeline_id: &str) -> PathBuf {
        // Sanitise: replace path separators so callers cannot escape the dir
        let safe_id = pipeline_id.replace(['/', '\\', '.', ':'], "_");
        self.dir.join(format!("{safe_id}.jsonl"))
    }
}

#[async_trait]
impl StoragePort for FileStorage {
    async fn store(&self, record: StorageRecord) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir).await.map_err(|e| {
            MyceliumError::Service(ServiceError::InvalidResponse(format!(
                "FileStorage: create_dir_all failed: {e}"
            )))
        })?;

        let path = self.pipeline_file(&record.pipeline_id);
        let mut line = serde_json::to_string(&record).map_err(|e| {
            MyceliumError::Service(ServiceError::InvalidResponse(format!(
                "FileStorage: serialise record failed: {e}"
            )))
        })?;
        line.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| {
                MyceliumError::Service(ServiceError::InvalidResponse(format!(
                    "FileStorage: open {}: {e}",
                    path.display()
                )))
            })?;

        file.write_all(line.as_bytes()).await.map_err(|e| {
            MyceliumError::Service(ServiceError::InvalidResponse(format!(
                "FileStorage: write failed: {e}"
            )))
        })?;

        Ok(())
    }

    async fn retrieve(&self, id: &str) -> Result<Option<StorageRecord>> {
        // Scan all .jsonl files — linear scan is acceptable for moderate volumes
        let Ok(mut dir) = tokio::fs::read_dir(&self.dir).await else {
            return Ok(None);
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            for line in content.lines() {
                if let Ok(record) = serde_json::from_str::<StorageRecord>(line)
                    && record.id == id
                {
                    return Ok(Some(record));
                }
            }
        }

        Ok(None)
    }

    async fn list(&self, pipeline_id: &str) -> Result<Vec<StorageRecord>> {
        let path = self.pipeline_file(pipeline_id);
        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            return Ok(vec![]);
        };

        let records = content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| serde_json::from_str::<StorageRecord>(line).ok())
            .collect();

        Ok(records)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        // Read-filter-rewrite strategy (adequate for typical pipeline sizes)
        let Ok(mut dir) = tokio::fs::read_dir(&self.dir).await else {
            return Ok(()); // dir does not exist → nothing to delete
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = tokio::fs::read_to_string(&path).await else {
                continue;
            };

            let (kept, found): (Vec<&str>, bool) = {
                let mut found = false;
                let kept = content
                    .lines()
                    .filter(|line| {
                        if let Ok(r) = serde_json::from_str::<StorageRecord>(line)
                            && r.id == id
                        {
                            found = true;
                            return false;
                        }
                        true
                    })
                    .collect::<Vec<_>>();
                (kept, found)
            };

            if found {
                let new_content = kept.join("\n");
                let new_content = if new_content.is_empty() {
                    new_content
                } else {
                    format!("{new_content}\n")
                };
                tokio::fs::write(&path, new_content.as_bytes())
                    .await
                    .map_err(|e| {
                        MyceliumError::Service(ServiceError::InvalidResponse(format!(
                            "FileStorage: rewrite after delete failed: {e}"
                        )))
                    })?;
                return Ok(());
            }
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PostgresStorage — feature = "postgres"
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "postgres")]
pub use postgres::PostgresStorage;

#[cfg(feature = "postgres")]
mod postgres {
    //! PostgreSQL-backed storage via sqlx.
    //!
    //! Assumes the following table exists:
    //!
    //! ```sql
    //! CREATE TABLE IF NOT EXISTS pipeline_records (
    //!     id          TEXT PRIMARY KEY,
    //!     pipeline_id TEXT NOT NULL,
    //!     node_name   TEXT NOT NULL,
    //!     data        JSONB NOT NULL,
    //!     metadata    JSONB NOT NULL DEFAULT '{}',
    //!     timestamp_ms BIGINT NOT NULL
    //! );
    //! CREATE INDEX IF NOT EXISTS idx_pipeline_records_pipeline_id
    //!     ON pipeline_records (pipeline_id);
    //! ```

    use crate::domain::error::{MyceliumError, Result, ServiceError};
    use crate::ports::storage::{StoragePort, StorageRecord};
    use sqlx::{PgPool, Row};

    /// `PostgreSQL` storage adapter.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::adapters::storage::PostgresStorage;
    /// use sqlx::PgPool;
    ///
    /// # tokio_test::block_on(async {
    /// let pool = PgPool::connect("postgres://localhost/mycelium").await.unwrap();
    /// let storage = PostgresStorage::new(pool);
    /// # });
    /// ```
    pub struct PostgresStorage {
        pool: PgPool,
    }

    impl PostgresStorage {
        /// Create a new [`PostgresStorage`] from a connection pool.
        ///
        /// # Example
        ///
        /// ```no_run
        /// use mycelium_graph::adapters::storage::PostgresStorage;
        /// use sqlx::PgPool;
        ///
        /// # tokio_test::block_on(async {
        /// let pool = PgPool::connect("postgres://localhost/mycelium").await.unwrap();
        /// let s = PostgresStorage::new(pool);
        /// # });
        /// ```
        pub const fn new(pool: PgPool) -> Self {
            Self { pool }
        }
    }

    #[async_trait::async_trait]
    impl StoragePort for PostgresStorage {
        async fn store(&self, record: StorageRecord) -> Result<()> {
            let metadata_json = serde_json::to_value(&record.metadata).map_err(|e| {
                MyceliumError::Service(ServiceError::InvalidResponse(format!(
                    "PostgresStorage: metadata serialise: {e}"
                )))
            })?;

            sqlx::query(
                "
                INSERT INTO pipeline_records
                    (id, pipeline_id, node_name, data, metadata, timestamp_ms)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (id) DO NOTHING
                ",
            )
            .bind(&record.id)
            .bind(&record.pipeline_id)
            .bind(&record.node_name)
            .bind(&record.data)
            .bind(metadata_json)
            .bind(i64::try_from(record.timestamp_ms).unwrap_or(i64::MAX))
            .execute(&self.pool)
            .await
            .map_err(|e| {
                MyceliumError::Service(ServiceError::InvalidResponse(format!(
                    "PostgresStorage: insert failed: {e}"
                )))
            })?;

            Ok(())
        }

        async fn retrieve(&self, id: &str) -> Result<Option<StorageRecord>> {
            let row = sqlx::query(
                "
                SELECT id, pipeline_id, node_name, data, metadata, timestamp_ms
                FROM pipeline_records
                WHERE id = $1
                ",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                MyceliumError::Service(ServiceError::InvalidResponse(format!(
                    "PostgresStorage: retrieve failed: {e}"
                )))
            })?;

            row.map_or(Ok(None), |r| {
                let metadata = serde_json::from_value(r.get::<serde_json::Value, _>("metadata"))
                    .unwrap_or_default();
                Ok(Some(StorageRecord {
                    id: r.get("id"),
                    pipeline_id: r.get("pipeline_id"),
                    node_name: r.get("node_name"),
                    data: r.get("data"),
                    metadata,
                    timestamp_ms: u64::try_from(r.get::<i64, _>("timestamp_ms")).unwrap_or(0),
                }))
            })
        }

        async fn list(&self, pipeline_id: &str) -> Result<Vec<StorageRecord>> {
            let rows = sqlx::query(
                "
                SELECT id, pipeline_id, node_name, data, metadata, timestamp_ms
                FROM pipeline_records
                WHERE pipeline_id = $1
                ORDER BY timestamp_ms ASC
                ",
            )
            .bind(pipeline_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                MyceliumError::Service(ServiceError::InvalidResponse(format!(
                    "PostgresStorage: list failed: {e}"
                )))
            })?;

            let records = rows
                .into_iter()
                .map(|r| {
                    let metadata =
                        serde_json::from_value(r.get::<serde_json::Value, _>("metadata"))
                            .unwrap_or_default();
                    StorageRecord {
                        id: r.get("id"),
                        pipeline_id: r.get("pipeline_id"),
                        node_name: r.get("node_name"),
                        data: r.get("data"),
                        metadata,
                        timestamp_ms: u64::try_from(r.get::<i64, _>("timestamp_ms")).unwrap_or(0),
                    }
                })
                .collect();

            Ok(records)
        }

        async fn delete(&self, id: &str) -> Result<()> {
            sqlx::query("DELETE FROM pipeline_records WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    MyceliumError::Service(ServiceError::InvalidResponse(format!(
                        "PostgresStorage: delete failed: {e}"
                    )))
                })?;

            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::{FileStorage, NullStorage};
    use crate::ports::storage::{StoragePort, StorageRecord};
    use serde_json::json;

    #[tokio::test]
    async fn null_storage_store_and_retrieve() {
        let s = NullStorage;
        let r = StorageRecord::new("p", "n", json!(null));
        s.store(r.clone()).await.unwrap();
        let got = s.retrieve(&r.id).await.unwrap();
        assert!(got.is_none(), "NullStorage must always return None");
    }

    #[tokio::test]
    async fn null_storage_list_and_delete_are_noops() {
        let s = NullStorage;
        let list = s.list("any").await.unwrap();
        assert!(list.is_empty());
        s.delete("any-id").await.unwrap();
    }

    #[tokio::test]
    async fn file_storage_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());

        let r = StorageRecord::new(
            "pipe-roundtrip",
            "fetch",
            json!({"url": "https://example.com"}),
        );
        let id = r.id.clone();

        storage.store(r).await.unwrap();

        let retrieved = storage.retrieve(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.pipeline_id, "pipe-roundtrip");
        assert_eq!(retrieved.node_name, "fetch");
    }

    #[tokio::test]
    async fn file_storage_list_scoped_to_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());

        storage
            .store(StorageRecord::new("pipe-a", "step1", json!(1)))
            .await
            .unwrap();
        storage
            .store(StorageRecord::new("pipe-a", "step2", json!(2)))
            .await
            .unwrap();
        storage
            .store(StorageRecord::new("pipe-b", "step1", json!(3)))
            .await
            .unwrap();

        let pipe_a = storage.list("pipe-a").await.unwrap();
        assert_eq!(pipe_a.len(), 2);

        let pipe_b = storage.list("pipe-b").await.unwrap();
        assert_eq!(pipe_b.len(), 1);
    }

    #[tokio::test]
    async fn file_storage_delete_removes_record() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());

        let r1 = StorageRecord::new("pipe-del", "n", json!(1));
        let r2 = StorageRecord::new("pipe-del", "n", json!(2));
        let id1 = r1.id.clone();

        storage.store(r1).await.unwrap();
        storage.store(r2).await.unwrap();

        storage.delete(&id1).await.unwrap();

        let records = storage.list("pipe-del").await.unwrap();
        assert_eq!(records.len(), 1);
        assert_ne!(records[0].id, id1);
    }

    #[tokio::test]
    async fn file_storage_retrieve_not_found_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());
        let result = storage.retrieve("no-such-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn file_storage_path_sanitises_separators() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());

        // pipeline_id with slashes should not escape the base directory
        let r = StorageRecord::new("../../etc/passwd", "n", json!(null));
        storage.store(r).await.unwrap();

        let files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        // File must be inside the temp dir, not some other directory
        assert_eq!(files.len(), 1);
        let fname = files[0].file_name();
        assert!(
            fname.to_string_lossy().contains("__"),
            "separators must be sanitised: got {fname:?}"
        );
    }

    #[tokio::test]
    async fn file_storage_retrieve_finds_correct_record() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());

        // Store records across two pipelines to exercise full-dir scan in retrieve
        let r1 = StorageRecord::new("pipe-x", "node-1", json!({"val": 1}));
        let r2 = StorageRecord::new("pipe-y", "node-2", json!({"val": 2}));
        let id1 = r1.id.clone();
        let id2 = r2.id.clone();

        storage.store(r1).await.unwrap();
        storage.store(r2).await.unwrap();

        let found = storage.retrieve(&id1).await.unwrap().unwrap();
        assert_eq!(found.id, id1);
        assert_eq!(found.pipeline_id, "pipe-x");

        let found2 = storage.retrieve(&id2).await.unwrap().unwrap();
        assert_eq!(found2.id, id2);
        assert_eq!(found2.pipeline_id, "pipe-y");
    }

    #[tokio::test]
    async fn file_storage_retrieve_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());
        // Store something so the dir exists and the scan loop runs
        storage
            .store(StorageRecord::new("p", "n", json!(0)))
            .await
            .unwrap();
        let result = storage.retrieve("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn file_storage_delete_nonexistent_dir_is_noop() {
        // Dir is never created — delete should return Ok without panicking
        let storage = FileStorage::new(std::path::PathBuf::from("/tmp/mycelium-no-such-dir-xyz"));
        storage.delete("any-id").await.unwrap();
    }

    #[tokio::test]
    async fn file_storage_delete_id_not_present_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());
        let r = StorageRecord::new("pipe-z", "n", json!(42));
        storage.store(r).await.unwrap();
        // Deleting a non-existent id should not modify the file
        storage.delete("totally-unknown-id").await.unwrap();
        let records = storage.list("pipe-z").await.unwrap();
        assert_eq!(records.len(), 1);
    }

    #[tokio::test]
    async fn file_storage_list_missing_pipeline_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path().to_path_buf());
        let records = storage.list("never-stored").await.unwrap();
        assert!(records.is_empty());
    }
}
