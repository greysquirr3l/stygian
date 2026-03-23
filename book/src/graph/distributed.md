# Distributed Execution

`stygian-graph` supports horizontal scaling via a Redis/Valkey-backed work queue.
Multiple worker processes can consume from the same queue, enabling throughput that
scales linearly with the number of nodes.

---

## Overview

In distributed mode, the `DistributedDagExecutor` splits pipeline execution across a
shared work queue:

```text
Producer process              Redis/Valkey               Worker processes
─────────────────             ──────────────             ─────────────────────
PipelineParser                                           DistributedDagExecutor
    ↓                                                          ↓
DagExecutor            ──── work items ────►          ServiceRegistry
    ↓                                                          ↓
IdempotencyKey         ◄─── results ────────          ScrapingService / AI
```

---

## Setup

### 1. Start Redis or Valkey

```bash
# Docker — Valkey (Redis-compatible, open-source fork)
docker run -d --name valkey -p 6379:6379 valkey/valkey:8

# Or Redis
docker run -d --name redis -p 6379:6379 redis:7
```

### 2. Enable the `redis` feature

```toml
# Cargo.toml
stygian-graph = { version = "0.1", features = ["redis"] }
```

### 3. Create a work queue and executor

```rust
use stygian_graph::adapters::distributed_redis::{RedisWorkQueue, RedisWorkQueueConfig};
use stygian_graph::adapters::distributed::DistributedDagExecutor;
use stygian_graph::application::registry::ServiceRegistry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = RedisWorkQueueConfig {
        url: "redis://localhost:6379".into(),
        ..Default::default()
    };
    let queue    = RedisWorkQueue::new(config).await?;
    let registry = ServiceRegistry::default_with_env()?;

    // Spawn 10 concurrent workers on this process
    let executor = DistributedDagExecutor::new(Arc::new(queue), registry, 10);

    // Execute a wave of tasks
    let results = executor
        .execute_wave("pipeline-id-1", tasks, &services)
        .await?;

    for result in results {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}
```

---

## Work queue operations

`RedisWorkQueue` implements the `WorkQueue` port trait:

```rust
pub trait WorkQueue: Send + Sync {
    /// Enqueue a task and return its unique ID.
    async fn enqueue(&self, task: Task) -> Result<TaskId>;

    /// Block until a task is available; returns `None` if queue is empty and closed.
    async fn dequeue(&self) -> Result<Option<Task>>;

    /// Acknowledge successful completion.
    async fn ack(&self, id: &TaskId) -> Result<()>;

    /// Return a task to the queue for another worker to retry.
    async fn nack(&self, id: &TaskId) -> Result<()>;
}
```

Tasks that are `nack`-ed (e.g. worker crashes mid-execution) are requeued automatically
after a visibility timeout.

---

## Idempotency in distributed mode

Every task carries an `IdempotencyKey`. Before executing, the worker checks a shared
idempotency store (backed by the same Redis instance):

- **Key not seen** — execute, store result under key.
- **Key already present** — return stored result immediately, skip execution.

This makes distributed task execution **safe to retry** — duplicate network deliveries
and worker restarts produce the same observable outcome.

```rust
use stygian_graph::domain::idempotency::IdempotencyKey;

// Deterministic key from pipeline id + input URL — replays the same result
let key = IdempotencyKey::from_input("pipeline-1", "https://example.com/item/42");
```

---

## Pipeline config for distributed mode

Enable distributed execution in a TOML pipeline:

```toml
[execution]
mode    = "distributed"
queue   = "redis://localhost:6379"
workers = 20

[[nodes]]
id      = "fetch"
service = "http"

[[nodes]]
id      = "extract"
service = "ai_claude"

[[edges]]
from = "fetch"
to   = "extract"
```

---

## Scaling tips

- **Worker count** — start at `num_cpus * 4` for I/O-heavy pipelines; reduce for
  CPU-heavy extraction workloads.
- **Redis connection pool** — the adapter maintains a pool internally; set
  `REDIS_MAX_CONNECTIONS` to tune (default `20`).
- **Backpressure** — the work queue has a configurable maximum depth. When full,
  `enqueue()` blocks the producer, preventing runaway memory growth.
- **Multiple queues** — use separate queue names per pipeline type to prevent high-volume
  crawls from starving high-priority extractions.
