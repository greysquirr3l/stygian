//! Worker pool executor with backpressure
//!
//! Provides a bounded worker pool for running `ScrapingService` tasks
//! with adaptive backpressure via tokio bounded channels.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::domain::executor::WorkerPool;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let pool = WorkerPool::new(4, 32);
//! pool.shutdown().await;
//! # });
//! ```

use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::domain::error::{GraphError, StygianError, Result};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

/// A work item sent to a pool worker
struct WorkItem {
    /// Service to invoke
    service: Arc<dyn ScrapingService>,
    /// Input for this invocation
    input: ServiceInput,
    /// One-shot channel to return the result
    reply: tokio::sync::oneshot::Sender<Result<ServiceOutput>>,
}

/// High-performance worker pool with bounded backpressure.
///
/// Distributes `ScrapingService` tasks across a fixed number of worker
/// Tokio tasks. When the internal channel is full, callers block until
/// a slot is available (backpressure).
///
/// Supports graceful shutdown via a `CancellationToken`.
pub struct WorkerPool {
    tx: mpsc::Sender<WorkItem>,
    cancel: CancellationToken,
    workers: Arc<Mutex<JoinSet<()>>>,
}

impl WorkerPool {
    /// Create a new worker pool.
    ///
    /// - `concurrency`: number of parallel worker tasks
    /// - `queue_depth`: bounded channel capacity (backpressure threshold)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::domain::executor::WorkerPool;
    ///
    /// let pool = WorkerPool::new(4, 32);
    /// ```
    #[allow(clippy::significant_drop_tightening)]
    pub fn new(concurrency: usize, queue_depth: usize) -> Self {
        let (tx, rx) = mpsc::channel::<WorkItem>(queue_depth);
        let rx = Arc::new(Mutex::new(rx));
        let cancel = CancellationToken::new();
        let mut join_set = JoinSet::new();

        for _ in 0..concurrency {
            let rx_clone = Arc::clone(&rx);
            let cancel_clone = cancel.clone();

            join_set.spawn(async move {
                loop {
                    // Check for cancellation before locking
                    if cancel_clone.is_cancelled() {
                        break;
                    }

                    let item = {
                        #[allow(clippy::significant_drop_tightening)]
                        let mut guard = rx_clone.lock().await;
                        tokio::select! {
                            biased;
                            () = cancel_clone.cancelled() => break,
                            item = guard.recv() => {
                                match item {
                                    Some(item) => item,
                                    None => break, // Channel closed
                                }
                            }
                        }
                    };

                    let result = item.service.execute(item.input).await;
                    // Ignore send error — caller may have dropped the receiver
                    let _ = item.reply.send(result);
                }
            });
        }

        Self {
            tx,
            cancel,
            workers: Arc::new(Mutex::new(join_set)),
        }
    }

    /// Submit a task to the pool.
    ///
    /// Blocks (async) if the internal queue is full (backpressure).
    ///
    /// # Errors
    ///
    /// Returns `GraphError::ExecutionFailed` if the pool has been shut down.
    pub async fn submit(
        &self,
        service: Arc<dyn ScrapingService>,
        input: ServiceInput,
    ) -> Result<ServiceOutput> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        self.tx
            .send(WorkItem {
                service,
                input,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                StygianError::Graph(GraphError::ExecutionFailed(
                    "Worker pool is shut down".into(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            StygianError::Graph(GraphError::ExecutionFailed(
                "Worker task dropped reply channel".into(),
            ))
        })?
    }

    /// Gracefully shut down the worker pool.
    ///
    /// Signals all workers to stop after their current task and waits
    /// for all worker tasks to complete.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        drop(self.tx); // Close sender so workers exit their recv loops

        let mut workers = self.workers.lock().await;
        while workers.join_next().await.is_some() {}
    }

    /// Returns the current backpressure state.
    ///
    /// `true` if the queue is at capacity and submitting will block.
    #[must_use]
    pub fn is_saturated(&self) -> bool {
        self.tx.capacity() == 0
    }

    /// Available capacity in the queue.
    pub fn available_capacity(&self) -> usize {
        self.tx.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::noop::NoopService;

    #[tokio::test]
    async fn test_worker_pool_basic_execution() {
        let pool = WorkerPool::new(2, 10);
        let svc: Arc<dyn ScrapingService> = Arc::new(NoopService);

        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: serde_json::json!({}),
        };

        let result = pool.submit(svc, input).await;
        assert!(result.is_ok());

        pool.shutdown().await;
    }

    #[tokio::test]
    async fn test_worker_pool_concurrent_tasks()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let pool = Arc::new(WorkerPool::new(4, 20));
        let svc: Arc<dyn ScrapingService> = Arc::new(NoopService);

        let mut handles = Vec::new();
        for i in 0..10 {
            let pool_clone = Arc::clone(&pool);
            let svc_clone = Arc::clone(&svc);
            handles.push(tokio::spawn(async move {
                let url = format!("https://example.com/{i}");
                let input = ServiceInput {
                    url,
                    params: serde_json::json!({}),
                };
                pool_clone.submit(svc_clone, input).await
            }));
        }

        for handle in handles {
            let result = handle.await?;
            assert!(result.is_ok(), "Task failed: {result:?}");
        }

        // Shut down: unwrap the Arc since we hold the only reference
        if let Some(p) = Arc::into_inner(pool) {
            p.shutdown().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_worker_pool_backpressure() {
        // Small queue: 1 slot, so second submit should block until first completes
        let pool = WorkerPool::new(1, 1);
        assert_eq!(pool.available_capacity(), 1);

        let svc: Arc<dyn ScrapingService> = Arc::new(NoopService);
        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: serde_json::json!({}),
        };

        let result = pool.submit(svc, input).await;
        assert!(result.is_ok());

        pool.shutdown().await;
    }

    #[tokio::test]
    async fn test_worker_pool_graceful_shutdown() {
        let pool = WorkerPool::new(2, 10);
        // Shutdown should complete without panicking even with no tasks
        pool.shutdown().await;
    }
}
