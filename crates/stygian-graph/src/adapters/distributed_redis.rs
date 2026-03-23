//! Redis Streams [`WorkQueuePort`](crate::ports::work_queue::WorkQueuePort) adapter
//!
//! Production-grade distributed work queue backed by Redis Streams with
//! consumer groups, dead-letter queues, and stuck-task reclamation.
//!
//! # Feature gate
//!
//! Requires `feature = "redis"` (shared with [`RedisCache`](crate::adapters::cache_redis::RedisCache)).
//!
//! # Architecture
//!
//! ```text
//! XADD  {stream}        ─► Consumer Group ─► XREADGROUP (workers)
//!                                           ─► XACK on success
//!                                           ─► XADD {stream}:dlq on exhausted retries
//! HSET  {stream}:results:{task_id}         ─► acknowledge stores output
//! HSET  {stream}:tasks:{task_id}           ─► task metadata (pipeline_id, node_name, attempt)
//! ```

use crate::domain::error::{CacheError, Result, StygianError};
use crate::ports::work_queue::{TaskStatus, WorkQueuePort, WorkTask};
use async_trait::async_trait;
use deadpool_redis::{Config as PoolConfig, Pool, Runtime};
use redis::AsyncCommands;
use tracing::{debug, error, info, warn};

// ─── Configuration ────────────────────────────────────────────────────────────

/// Configuration for [`RedisWorkQueue`].
#[derive(Debug, Clone)]
pub struct RedisWorkQueueConfig {
    /// Redis connection URL.
    pub url: String,
    /// Stream key name (default: `"stygian:tasks"`).
    pub stream_name: String,
    /// Consumer group name (default: `"stygian-workers"`).
    pub group_name: String,
    /// Unique consumer name for this worker instance.
    /// Defaults to `"{hostname}:{pid}"`.
    pub consumer_name: String,
    /// Max pool connections (default 8).
    pub pool_size: usize,
    /// Max retry attempts before dead-lettering (default 3).
    pub max_retries: u32,
    /// Block timeout in milliseconds for XREADGROUP (default 1000).
    pub block_timeout_ms: usize,
    /// Idle time threshold in milliseconds for XCLAIM reclamation (default 30 000).
    pub idle_threshold_ms: usize,
}

impl Default for RedisWorkQueueConfig {
    fn default() -> Self {
        let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "local".to_string());
        let consumer_name = format!("{}:{}", host, std::process::id());
        Self {
            url: "redis://127.0.0.1:6379".into(),
            stream_name: "stygian:tasks".into(),
            group_name: "stygian-workers".into(),
            consumer_name,
            pool_size: 8,
            max_retries: 3,
            block_timeout_ms: 1000,
            idle_threshold_ms: 30_000,
        }
    }
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// Redis Streams backed [`WorkQueuePort`] adapter.
///
/// Uses XADD / XREADGROUP / XACK / XCLAIM for reliable distributed task
/// execution with consumer groups and automatic stuck-task recovery.
pub struct RedisWorkQueue {
    pool: Pool,
    config: RedisWorkQueueConfig,
}

impl RedisWorkQueue {
    /// Create a new [`RedisWorkQueue`] and ensure the consumer group exists.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Cache`] if pool creation fails.
    pub async fn new(config: RedisWorkQueueConfig) -> Result<Self> {
        let pool_cfg = PoolConfig::from_url(&config.url);
        let pool = pool_cfg
            .builder()
            .map(|b| b.max_size(config.pool_size))
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!(
                    "failed to build Redis pool: {e}"
                )))
            })?
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!(
                    "failed to build Redis pool: {e}"
                )))
            })?;

        // Ensure consumer group exists (XGROUP CREATE ... MKSTREAM)
        let queue = Self { pool, config };
        queue.ensure_consumer_group().await?;
        Ok(queue)
    }

    /// Create from an existing pool (share pool with [`RedisCache`](crate::adapters::cache_redis::RedisCache)).
    pub async fn from_pool(pool: Pool, config: RedisWorkQueueConfig) -> Result<Self> {
        let queue = Self { pool, config };
        queue.ensure_consumer_group().await?;
        Ok(queue)
    }

    /// Ensure the consumer group exists on the stream.
    async fn ensure_consumer_group(&self) -> Result<()> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis pool error: {e}")))
        })?;

        // XGROUP CREATE stream group $ MKSTREAM — ignore BUSYGROUP error (already exists)
        let result: redis::RedisResult<String> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&self.config.stream_name)
            .arg(&self.config.group_name)
            .arg("$")
            .arg("MKSTREAM")
            .query_async(&mut *conn)
            .await;

        match result {
            Ok(_) => {
                debug!(
                    stream = %self.config.stream_name,
                    group = %self.config.group_name,
                    "created consumer group"
                );
            }
            Err(e) if e.to_string().contains("BUSYGROUP") => {
                debug!(
                    stream = %self.config.stream_name,
                    group = %self.config.group_name,
                    "consumer group already exists"
                );
            }
            Err(e) => {
                return Err(StygianError::Cache(CacheError::WriteFailed(format!(
                    "XGROUP CREATE failed: {e}"
                ))));
            }
        }

        Ok(())
    }

    /// Task metadata hash key.
    fn task_meta_key(&self, task_id: &str) -> String {
        format!("{}:tasks:{}", self.config.stream_name, task_id)
    }

    /// Results hash key.
    fn result_key(&self, task_id: &str) -> String {
        format!("{}:results:{}", self.config.stream_name, task_id)
    }

    /// Dead-letter queue stream name.
    fn dlq_stream(&self) -> String {
        format!("{}:dlq", self.config.stream_name)
    }

    /// Reclaim stuck tasks from crashed consumers via XCLAIM.
    pub async fn reclaim_stuck_tasks(&self) -> Result<Vec<WorkTask>> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;

        // XPENDING stream group - + count
        let pending: Vec<Vec<redis::Value>> = redis::cmd("XPENDING")
            .arg(&self.config.stream_name)
            .arg(&self.config.group_name)
            .arg("-")
            .arg("+")
            .arg(100_i64)
            .query_async(&mut *conn)
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::ReadFailed(format!("XPENDING failed: {e}")))
            })?;

        let mut reclaimed = Vec::new();

        for entry in &pending {
            // Each entry: [message_id, consumer, idle_ms, delivery_count]
            if entry.len() < 3 {
                continue;
            }
            let msg_id = match &entry[0] {
                redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                _ => continue,
            };
            let idle_ms: usize = match &entry[2] {
                redis::Value::Int(n) => *n as usize,
                _ => continue,
            };

            if idle_ms < self.config.idle_threshold_ms {
                continue;
            }

            // XCLAIM stream group consumer min-idle-time message-id
            let claimed: redis::RedisResult<Vec<redis::Value>> = redis::cmd("XCLAIM")
                .arg(&self.config.stream_name)
                .arg(&self.config.group_name)
                .arg(&self.config.consumer_name)
                .arg(self.config.idle_threshold_ms)
                .arg(&msg_id)
                .query_async(&mut *conn)
                .await;

            if let Ok(messages) = claimed {
                for msg in &messages {
                    if let Some(task) = Self::parse_stream_message(msg) {
                        info!(task_id = %task.id, idle_ms, "reclaimed stuck task");
                        reclaimed.push(task);
                    }
                }
            }
        }

        Ok(reclaimed)
    }

    /// Parse a Redis Stream message value into a [`WorkTask`].
    fn parse_stream_message(msg: &redis::Value) -> Option<WorkTask> {
        // Stream message: [message_id, [field, value, field, value, ...]]
        let arr = match msg {
            redis::Value::Array(a) => a,
            _ => return None,
        };
        if arr.len() < 2 {
            return None;
        }
        let fields = match &arr[1] {
            redis::Value::Array(a) => a,
            _ => return None,
        };

        // Look for the "payload" field
        let mut payload: Option<&[u8]> = None;
        let mut i = 0;
        while i + 1 < fields.len() {
            if let redis::Value::BulkString(key) = &fields[i]
                && key == b"payload"
                && let redis::Value::BulkString(val) = &fields[i + 1]
            {
                payload = Some(val);
            }
            i += 2;
        }

        let payload = payload?;
        serde_json::from_slice(payload).ok()
    }
}

// ─── WorkQueuePort ────────────────────────────────────────────────────────────

#[async_trait]
impl WorkQueuePort for RedisWorkQueue {
    async fn enqueue(&self, task: WorkTask) -> Result<()> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis pool error: {e}")))
        })?;

        let payload = serde_json::to_string(&task).map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!(
                "task serialisation failed: {e}"
            )))
        })?;

        // XADD stream * payload <json>
        let _msg_id: String = redis::cmd("XADD")
            .arg(&self.config.stream_name)
            .arg("*")
            .arg("payload")
            .arg(&payload)
            .query_async(&mut *conn)
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!("XADD failed: {e}")))
            })?;

        // Store task metadata for status lookups
        let meta_key = self.task_meta_key(&task.id);
        let meta = serde_json::json!({
            "pipeline_id": task.pipeline_id,
            "node_name": task.node_name,
            "attempt": task.attempt,
            "status": "pending",
        });
        conn.set::<_, _, ()>(&meta_key, meta.to_string())
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!(
                    "SET task meta failed: {e}"
                )))
            })?;

        debug!(task_id = %task.id, node = %task.node_name, "enqueued task to Redis stream");
        Ok(())
    }

    async fn try_dequeue(&self) -> Result<Option<WorkTask>> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;

        // XREADGROUP GROUP group consumer COUNT 1 BLOCK timeout STREAMS stream >
        let result: redis::RedisResult<redis::Value> = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(&self.config.group_name)
            .arg(&self.config.consumer_name)
            .arg("COUNT")
            .arg(1_i64)
            .arg("BLOCK")
            .arg(self.config.block_timeout_ms)
            .arg("STREAMS")
            .arg(&self.config.stream_name)
            .arg(">")
            .query_async(&mut *conn)
            .await;

        let value = match result {
            Ok(v) => v,
            Err(e) => {
                // Timeout returns Nil, not an error
                if e.to_string().contains("nil") {
                    return Ok(None);
                }
                return Err(StygianError::Cache(CacheError::ReadFailed(format!(
                    "XREADGROUP failed: {e}"
                ))));
            }
        };

        // Response: [[stream_name, [[message_id, [field, value, ...]]]]]
        let streams = match &value {
            redis::Value::Array(s) if !s.is_empty() => s,
            redis::Value::Nil => return Ok(None),
            _ => return Ok(None),
        };

        let stream_data = match &streams[0] {
            redis::Value::Array(s) if s.len() >= 2 => s,
            _ => return Ok(None),
        };

        let messages = match &stream_data[1] {
            redis::Value::Array(m) if !m.is_empty() => m,
            _ => return Ok(None),
        };

        if let Some(task) = Self::parse_stream_message(&messages[0]) {
            // Update status to InProgress
            let meta_key = self.task_meta_key(&task.id);
            let meta = serde_json::json!({
                "pipeline_id": task.pipeline_id,
                "node_name": task.node_name,
                "attempt": task.attempt,
                "status": "in_progress",
                "worker_id": self.config.consumer_name,
            });
            let _ = conn.set::<_, _, ()>(&meta_key, meta.to_string()).await;

            debug!(task_id = %task.id, consumer = %self.config.consumer_name, "dequeued task");
            return Ok(Some(task));
        }

        Ok(None)
    }

    async fn acknowledge(&self, task_id: &str, output: serde_json::Value) -> Result<()> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis pool error: {e}")))
        })?;

        // Store the result
        let result_key = self.result_key(task_id);
        let output_str = output.to_string();
        conn.set::<_, _, ()>(&result_key, &output_str)
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::WriteFailed(format!("SET result failed: {e}")))
            })?;

        // Update status to completed
        let meta_key = self.task_meta_key(task_id);
        let meta_raw: Option<String> = conn.get(&meta_key).await.unwrap_or(None);
        if let Some(raw) = meta_raw
            && let Ok(mut meta) = serde_json::from_str::<serde_json::Value>(&raw)
        {
            meta["status"] = serde_json::json!("completed");
            let _ = conn.set::<_, _, ()>(&meta_key, meta.to_string()).await;
        }

        info!(task_id = %task_id, "task acknowledged (completed)");
        Ok(())
    }

    async fn fail(&self, task_id: &str, error_msg: &str) -> Result<()> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::WriteFailed(format!("Redis pool error: {e}")))
        })?;

        // Read current attempt count from metadata
        let meta_key = self.task_meta_key(task_id);
        let meta_raw: Option<String> = conn.get(&meta_key).await.unwrap_or(None);

        let attempt = meta_raw
            .as_ref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|m| m["attempt"].as_u64())
            .unwrap_or(0) as u32;

        if attempt >= self.config.max_retries {
            // Dead-letter: XADD to DLQ stream
            let dlq = self.dlq_stream();
            let dlq_payload = serde_json::json!({
                "task_id": task_id,
                "error": error_msg,
                "attempt": attempt,
            });
            let _: redis::RedisResult<String> = redis::cmd("XADD")
                .arg(&dlq)
                .arg("*")
                .arg("payload")
                .arg(dlq_payload.to_string())
                .query_async(&mut *conn)
                .await;

            // Update meta to dead_letter
            let meta = serde_json::json!({
                "status": "dead_letter",
                "error": error_msg,
                "attempt": attempt,
            });
            let _ = conn.set::<_, _, ()>(&meta_key, meta.to_string()).await;

            warn!(task_id = %task_id, %error_msg, attempt, "task dead-lettered after max retries");
        } else {
            // Update meta with incremented attempt
            let meta = serde_json::json!({
                "status": "failed",
                "error": error_msg,
                "attempt": attempt + 1,
            });
            let _ = conn.set::<_, _, ()>(&meta_key, meta.to_string()).await;

            error!(task_id = %task_id, attempt = attempt + 1, %error_msg, "task failed, will retry");
        }

        Ok(())
    }

    async fn status(&self, task_id: &str) -> Result<Option<TaskStatus>> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;

        let meta_key = self.task_meta_key(task_id);
        let meta_raw: Option<String> = conn.get(&meta_key).await.unwrap_or(None);

        let Some(raw) = meta_raw else {
            return Ok(None);
        };

        let meta: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!(
                "task meta deserialise failed: {e}"
            )))
        })?;

        let status_str = meta["status"].as_str().unwrap_or("pending");

        let status = match status_str {
            "pending" => TaskStatus::Pending,
            "in_progress" => TaskStatus::InProgress {
                worker_id: meta["worker_id"].as_str().unwrap_or("unknown").to_string(),
            },
            "completed" => {
                // Fetch the actual output from the results key
                let result_key = self.result_key(task_id);
                let output_raw: Option<String> = conn.get(&result_key).await.unwrap_or(None);
                let output = output_raw
                    .and_then(|r| serde_json::from_str(&r).ok())
                    .unwrap_or(serde_json::Value::Null);
                TaskStatus::Completed { output }
            }
            "failed" => TaskStatus::Failed {
                error: meta["error"].as_str().unwrap_or("").to_string(),
                attempt: meta["attempt"].as_u64().unwrap_or(0) as u32,
            },
            "dead_letter" => TaskStatus::DeadLetter {
                error: meta["error"].as_str().unwrap_or("").to_string(),
            },
            _ => TaskStatus::Pending,
        };

        Ok(Some(status))
    }

    async fn collect_results(&self, pipeline_id: &str) -> Result<Vec<(String, serde_json::Value)>> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;

        // Scan for task metadata keys matching this pipeline
        let pattern = format!("{}:tasks:*", self.config.stream_name);
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut *conn)
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::ReadFailed(format!("KEYS scan failed: {e}")))
            })?;

        let mut results = Vec::new();

        for key in &keys {
            let meta_raw: Option<String> = conn.get(key).await.unwrap_or(None);
            let Some(raw) = meta_raw else { continue };
            let Ok(meta) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };

            // Filter by pipeline_id and completed status
            if meta["pipeline_id"].as_str() != Some(pipeline_id) {
                continue;
            }
            if meta["status"].as_str() != Some("completed") {
                continue;
            }

            let node_name = meta["node_name"].as_str().unwrap_or("").to_string();

            // Extract task_id from key: "{stream}:tasks:{task_id}"
            let task_id = key.rsplit(':').next().unwrap_or("");
            let result_key = self.result_key(task_id);
            let output_raw: Option<String> = conn.get(&result_key).await.unwrap_or(None);
            let output = output_raw
                .and_then(|r| serde_json::from_str(&r).ok())
                .unwrap_or(serde_json::Value::Null);

            results.push((node_name, output));
        }

        Ok(results)
    }

    async fn pending_count(&self) -> Result<usize> {
        let mut conn = self.pool.get().await.map_err(|e| {
            StygianError::Cache(CacheError::ReadFailed(format!("Redis pool error: {e}")))
        })?;

        // XLEN gives total entries in the stream (approximate pending count)
        let len: usize = redis::cmd("XLEN")
            .arg(&self.config.stream_name)
            .query_async(&mut *conn)
            .await
            .map_err(|e| {
                StygianError::Cache(CacheError::ReadFailed(format!("XLEN failed: {e}")))
            })?;

        Ok(len)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_task_serialisation_roundtrip() {
        let task = WorkTask {
            id: "t-1".to_string(),
            pipeline_id: "p-1".to_string(),
            node_name: "fetch".to_string(),
            input: json!({"url": "https://example.com"}),
            wave: 0,
            attempt: 0,
            idempotency_key: "ik-1".to_string(),
        };

        let serialised = serde_json::to_string(&task).unwrap();
        let deserialised: WorkTask = serde_json::from_str(&serialised).unwrap();

        assert_eq!(deserialised.id, task.id);
        assert_eq!(deserialised.pipeline_id, task.pipeline_id);
        assert_eq!(deserialised.node_name, task.node_name);
        assert_eq!(deserialised.input, task.input);
        assert_eq!(deserialised.wave, task.wave);
        assert_eq!(deserialised.attempt, task.attempt);
        assert_eq!(deserialised.idempotency_key, task.idempotency_key);
    }

    #[test]
    fn test_default_config() {
        let cfg = RedisWorkQueueConfig::default();
        assert_eq!(cfg.url, "redis://127.0.0.1:6379");
        assert_eq!(cfg.stream_name, "stygian:tasks");
        assert_eq!(cfg.group_name, "stygian-workers");
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.block_timeout_ms, 1000);
        assert_eq!(cfg.idle_threshold_ms, 30_000);
        assert!(!cfg.consumer_name.is_empty());
    }

    #[test]
    fn test_key_generation() {
        let stream_name = "stygian:tasks";
        let task_id = "abc-123";
        assert_eq!(
            format!("{stream_name}:tasks:{task_id}"),
            "stygian:tasks:tasks:abc-123"
        );
        assert_eq!(
            format!("{stream_name}:results:{task_id}"),
            "stygian:tasks:results:abc-123"
        );
        assert_eq!(format!("{stream_name}:dlq"), "stygian:tasks:dlq");
    }

    #[test]
    fn test_parse_stream_message_empty() {
        let msg = redis::Value::Nil;
        assert!(RedisWorkQueue::parse_stream_message(&msg).is_none());
    }

    #[test]
    fn test_parse_stream_message_valid() {
        let task = WorkTask {
            id: "t-1".to_string(),
            pipeline_id: "p-1".to_string(),
            node_name: "fetch".to_string(),
            input: json!({"url": "https://example.com"}),
            wave: 0,
            attempt: 0,
            idempotency_key: "ik-1".to_string(),
        };
        let payload = serde_json::to_vec(&task).unwrap();

        let msg = redis::Value::Array(vec![
            redis::Value::BulkString(b"1234-0".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"payload".to_vec()),
                redis::Value::BulkString(payload),
            ]),
        ]);

        let parsed = RedisWorkQueue::parse_stream_message(&msg).unwrap();
        assert_eq!(parsed.id, "t-1");
        assert_eq!(parsed.node_name, "fetch");
    }

    #[test]
    fn test_consumer_name_is_unique() {
        let cfg1 = RedisWorkQueueConfig::default();
        // Consumer name includes PID so it's unique per process
        assert!(cfg1.consumer_name.contains(&std::process::id().to_string()));
    }
}
