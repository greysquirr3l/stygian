//! Work queue port — distributed task execution interface
//!
//! Defines the abstractions required to distribute pipeline node execution
//! across multiple workers (local threads, remote processes, or serverless
//! workers such as Spin components).
//!
//! # Architecture
//!
//! ```text
//! DagExecutor
//!     │  (wave N ready)
//!     ▼
//! WorkQueuePort::enqueue(tasks…)
//!     │
//!     ├─► Worker 1: dequeue → execute node → acknowledge
//!     ├─► Worker 2: dequeue → execute node → acknowledge
//!     └─► Worker N: dequeue → execute node → acknowledge
//!     │
//! collect_results(pipeline_id) → Vec<(node_name, ServiceOutput)>
//! ```

use crate::domain::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Domain types
// ─────────────────────────────────────────────────────────────────────────────

/// A single unit of work: one pipeline node that needs to be executed.
///
/// Tasks are serialisable so they can be transmitted to remote workers via
/// Redis Streams, Kafka, HTTP, or any other transport.
///
/// # Example
///
/// ```
/// use mycelium_graph::ports::work_queue::WorkTask;
/// use serde_json::json;
///
/// let task = WorkTask {
///     id: "01HX...".to_string(),
///     pipeline_id: "pipeline-abc".to_string(),
///     node_name: "fetch-homepage".to_string(),
///     input: json!({"url": "https://example.com"}),
///     wave: 0,
///     attempt: 0,
///     idempotency_key: "ik-01HX".to_string(),
/// };
/// assert_eq!(task.node_name, "fetch-homepage");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkTask {
    /// Unique task identifier (ULID recommended)
    pub id: String,
    /// Pipeline this task belongs to
    pub pipeline_id: String,
    /// Name of the DAG node to execute
    pub node_name: String,
    /// Input data for the node's service (serialised as JSON)
    pub input: serde_json::Value,
    /// Execution wave (tasks in the same wave are independent and can run in
    /// parallel)
    pub wave: u32,
    /// Retry attempt number (0 = first attempt)
    pub attempt: u32,
    /// Idempotency key for safe retries
    pub idempotency_key: String,
}

/// Lifecycle status of a [`WorkTask`].
///
/// # Example
///
/// ```
/// use mycelium_graph::ports::work_queue::TaskStatus;
///
/// let status = TaskStatus::Pending;
/// assert!(matches!(status, TaskStatus::Pending));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is queued, not yet claimed by a worker
    Pending,
    /// A worker has claimed the task and is executing it
    InProgress {
        /// Identifier of the worker processing this task
        worker_id: String,
    },
    /// Task completed successfully
    Completed {
        /// Output produced by the service (serialised as JSON)
        output: serde_json::Value,
    },
    /// Task failed; will be retried if `attempt < max_attempts`
    Failed {
        /// Human-readable error message
        error: String,
        /// Which attempt number failed
        attempt: u32,
    },
    /// Task has exhausted all retries and moved to the dead-letter queue
    DeadLetter {
        /// Final error message
        error: String,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Port trait
// ─────────────────────────────────────────────────────────────────────────────

/// Port: distributed work queue for pipeline node execution.
///
/// Implementations can range from an in-process [`VecDeque`] (for local
/// single-node setups) to Redis Streams, Kafka, or Spin KV store for
/// multi-worker deployments.
///
/// [`VecDeque`]: std::collections::VecDeque
///
/// # Example
///
/// ```
/// use mycelium_graph::ports::work_queue::{WorkTask, WorkQueuePort};
/// use mycelium_graph::adapters::distributed::LocalWorkQueue;
/// use serde_json::json;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let queue = LocalWorkQueue::new();
/// let task = WorkTask {
///     id: "t1".to_string(),
///     pipeline_id: "p1".to_string(),
///     node_name: "fetch".to_string(),
///     input: json!({"url": "https://example.com"}),
///     wave: 0,
///     attempt: 0,
///     idempotency_key: "ik-t1".to_string(),
/// };
/// queue.enqueue(task).await.unwrap();
/// let dequeued = queue.try_dequeue().await.unwrap();
/// assert!(dequeued.is_some());
/// # });
/// ```
#[async_trait]
pub trait WorkQueuePort: Send + Sync {
    /// Enqueue a task for execution.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::ports::work_queue::{WorkTask, WorkQueuePort};
    /// use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// use serde_json::json;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let queue = LocalWorkQueue::new();
    /// let task = WorkTask {
    ///     id: "t1".to_string(),
    ///     pipeline_id: "p1".to_string(),
    ///     node_name: "fetch".to_string(),
    ///     input: json!({"url": "https://example.com"}),
    ///     wave: 0,
    ///     attempt: 0,
    ///     idempotency_key: "ik-t1".to_string(),
    /// };
    /// queue.enqueue(task).await.unwrap();
    /// # });
    /// ```
    async fn enqueue(&self, task: WorkTask) -> Result<()>;

    /// Attempt to dequeue one task. Returns `None` immediately if the queue is
    /// empty (non-blocking).
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::ports::work_queue::{WorkTask, WorkQueuePort};
    /// use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// use serde_json::json;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let queue = LocalWorkQueue::new();
    /// let result = queue.try_dequeue().await.unwrap();
    /// assert!(result.is_none()); // empty
    /// # });
    /// ```
    async fn try_dequeue(&self) -> Result<Option<WorkTask>>;

    /// Acknowledge successful completion of a task.
    ///
    /// Records the output and marks the task as [`TaskStatus::Completed`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::work_queue::WorkQueuePort;
    /// # use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// # use serde_json::json;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let queue = LocalWorkQueue::new();
    /// queue.acknowledge("task-id", json!({"data": "ok"})).await.unwrap();
    /// # });
    /// ```
    async fn acknowledge(&self, task_id: &str, output: serde_json::Value) -> Result<()>;

    /// Record a task failure, potentially retrying or dead-lettering it.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::work_queue::WorkQueuePort;
    /// # use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let queue = LocalWorkQueue::new();
    /// queue.fail("task-id", "connection refused").await.unwrap();
    /// # });
    /// ```
    async fn fail(&self, task_id: &str, error: &str) -> Result<()>;

    /// Retrieve the current [`TaskStatus`] for a task by ID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::work_queue::WorkQueuePort;
    /// # use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let queue = LocalWorkQueue::new();
    /// let status = queue.status("task-id").await.unwrap();
    /// # });
    /// ```
    async fn status(&self, task_id: &str) -> Result<Option<TaskStatus>>;

    /// Collect all completed results for a pipeline.
    ///
    /// Returns `(node_name, output)` pairs for every completed task
    /// belonging to `pipeline_id`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mycelium_graph::ports::work_queue::WorkQueuePort;
    /// # use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let queue = LocalWorkQueue::new();
    /// let results = queue.collect_results("pipeline-abc").await.unwrap();
    /// # });
    /// ```
    async fn collect_results(&self, pipeline_id: &str) -> Result<Vec<(String, serde_json::Value)>>;

    /// Number of tasks currently in the pending queue.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::adapters::distributed::LocalWorkQueue;
    /// use mycelium_graph::ports::work_queue::WorkQueuePort;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let queue = LocalWorkQueue::new();
    /// assert_eq!(queue.pending_count().await.unwrap(), 0);
    /// # });
    /// ```
    async fn pending_count(&self) -> Result<usize>;
}
