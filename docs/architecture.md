# Stygian Architecture

Stygian is a high-performance, graph-based scraping engine that treats pipelines as Directed
Acyclic Graphs (DAGs). It is built around the **Hexagonal Architecture** (Ports & Adapters)
pattern to enable extreme concurrency, testability, and extensibility.

---

## Layer Overview

```text
┌──────────────────────────────────────────────────────┐
│                   CLI / Entry Points                 │
│              (src/bin/stygian.rs)                   │
├──────────────────────────────────────────────────────┤
│                  Application Layer                   │
│   registry · pipeline_parser · metrics · health      │
├──────────────────────────────────────────────────────┤
│                    Domain Core                       │
│       graph · executor · pipeline · idempotency      │
├──────────────────────────────────────────────────────┤
│                      Ports                           │
│  ScrapingService · AIProvider · CachePort · ...      │
├──────────────────────────────────────────────────────┤
│                    Adapters                          │
│   http · claude · openai · gemini · browser · ...    │
└──────────────────────────────────────────────────────┘
```

The dependency arrow points **inward**:

```text
CLI → Application → Domain ← Ports ← Adapters
```

The domain never depends on adapters. All external capabilities are declared as port traits; adapters implement those traits and are injected at runtime.

---

## Domain Layer (`src/domain/`)

The domain is **pure Rust** with zero I/O dependencies. It may only import from `std`, `serde`,
and other pure data crates.

### Key types

| Type | File | Purpose |
| --- | --- | --- |
| `Pipeline` | `domain/graph.rs` | Graph of `Node`s and `Edge`s; creates a petgraph DAG |
| `DagExecutor` | `domain/graph.rs` | Topological sort → wave-based concurrent execution |
| `WorkerPool` | `domain/executor.rs` | Bounded tokio worker pool with backpressure |
| `IdempotencyKey` | `domain/idempotency.rs` | ULID-based key for safe retries |

### Pipeline typestate

Pipelines use the typestate pattern to enforce correct lifecycle at compile time:

```rust
use stygian_graph::domain::pipeline::{
    PipelineUnvalidated, PipelineValidated, PipelineExecuting, PipelineComplete,
};

let p: PipelineUnvalidated = PipelineUnvalidated::new("my-pipeline", metadata);
let p: PipelineValidated   = p.validate()?;     // compile-time safe
let p: PipelineExecuting   = p.execute();        // must be validated first
let p: PipelineComplete    = p.complete(result); // must be executing
```

The compiler rejects out-of-order transitions. Shared phantom-type states carry zero runtime overhead.

---

## Ports Layer (`src/ports.rs`)

Ports are **trait definitions**. They are the only interface the domain accepts.  
No adapter code ever leaks into the domain.

### Core ports

```rust
// Primary service port — any scraper must implement this
pub trait ScrapingService: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput>;
}

// Intelligence port — route to any LLM provider
pub trait AIProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;
    async fn extract(&self, prompt: String, schema: Value) -> Result<Value>;
    async fn stream_extract(&self, prompt: String, schema: Value)
        -> Result<BoxStream<'static, Result<Value>>>;
}

// Caching port — decouple cache implementation from domain
pub trait CachePort: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> Result<()>;
    async fn invalidate(&self, key: &str) -> Result<()>;
    async fn exists(&self, key: &str) -> Result<bool>;
}

// Resilience port — circuit breaker abstraction
pub trait CircuitBreaker: Send + Sync {
    fn state(&self) -> CircuitState;
    fn record_success(&self);
    fn record_failure(&self);
    fn attempt_reset(&self) -> bool;
}

// Rate limiting port
pub trait RateLimiter: Send + Sync {
    async fn check_rate_limit(&self, key: &str) -> Result<bool>;
    async fn record_request(&self, key: &str) -> Result<()>;
}
```

---

## Adapters Layer (`src/adapters/`)

Adapters implement port traits and handle real I/O. They are **never** imported by the domain.

| Adapter | Port | Notes |
| --- | --- | --- |
| `HttpAdapter` | `ScrapingService` | reqwest, UA rotation, cookie jar, retry |
| `BrowserAdapter` | `ScrapingService` | chromiumoxide via stygian-browser |
| `ClaudeProvider` | `AIProvider` | Anthropic API, streaming |
| `OpenAiProvider` | `AIProvider` | OpenAI Chat API |
| `GeminiProvider` | `AIProvider` | Google Gemini API |
| `CopilotProvider` | `AIProvider` | GitHub Copilot API |
| `OllamaProvider` | `AIProvider` | Local LLM via Ollama |
| `BoundedLruCache` | `CachePort` | LRU eviction, `NonZeroUsize` capacity |
| `DashMapCache` | `CachePort` | Concurrent hash map with TTL cleanup |
| `CircuitBreakerImpl` | `CircuitBreaker` | Sliding-window failure threshold |
| `NoopCircuitBreaker` | `CircuitBreaker` | Passthrough, useful for testing |
| `NoopRateLimiter` | `RateLimiter` | Always allows, useful for testing |

### Adding a new adapter

1. Identify the port trait your adapter will satisfy (e.g., `ScrapingService`).  
2. Create `src/adapters/my_adapter.rs`.
3. Implement the trait using `#[async_trait::async_trait]`.
4. Re-export from `src/adapters.rs`.
5. Register in the `ServiceRegistry` at startup.

No changes to domain code are ever required.

---

## Application Layer (`src/application/`)

The application layer **orchestrates** domain logic using injected adapters.  
It may import from both domain and ports, but never directly from adapters.

| Module | Purpose |
| --- | --- |
| `registry` | `ServiceRegistry` — runtime `name → Arc<dyn ScrapingService>` map |
| `pipeline_parser` | TOML → `PipelineDefinition`; layered config with figment |
| `executor` | `PipelineRunner` — wires `ServiceRegistry` to `DagExecutor` |
| `extraction` | `ExtractionService` — orchestrates AI provider selection |
| `schema_discovery` | `SchemaDiscovery` — infers JSON schema via LLM |
| `metrics` | `MetricsRegistry` — atomic counters + Prometheus text export |
| `health` | `HealthReporter` — liveness/readiness gates |
| `config` | `AppConfig` — environment + file layered config |

---

## Concurrency Model

### Wave-based DAG execution

`DagExecutor` runs nodes in topological waves. All nodes in a wave are co-independent (no
intra-wave edges), so they execute concurrently via `tokio::spawn`. The executor waits for
every node in the current wave before starting the next.

```text
Wave 0: [fetch]
Wave 1: [parse-a, parse-b]   ← concurrent
Wave 2: [merge]
Wave 3: [store]
```

### Worker pool with backpressure

`WorkerPool` wraps a bounded `tokio::sync::mpsc` channel. Producers block when the queue
is full, enforcing natural backpressure. Workers run on a multi-thread Tokio runtime.
Shutdown is cooperative via `CancellationToken`.

```rust
let pool = WorkerPool::new(
    concurrency,  // number of concurrent workers
    queue_depth,  // max items waiting in queue (backpressure point)
);
let output = pool.submit(service, input).await?;
pool.shutdown().await; // drains queue gracefully
```

### Rayon for CPU-bound work

Heavy CPU work (schema extraction, content parsing) uses Rayon. Call `tokio::task::spawn_blocking(|| rayon_heavy_job())` at the boundary to avoid blocking async threads.

---

## Security Patterns

- **Fail-secure by default**: missing config → error, not silent fallback.
- **Authorization at the repository level**: `ServiceRegistry` can enforce access
    control on service names before dispatch.
- **No secrets in logs**: credentials flow through environment variables only.
- **Idempotency keys**: all retryable operations carry ULID idempotency keys stored in
    the cache to prevent duplicate side effects.

```rust
let key = IdempotencyKey::new();             // generates a new ULID
store.claim(&key, ttl).await?;               // atomic claim
// … do work …
store.store(&key, result_json, ttl).await?;  // persist result
```

---

## Rust Patterns Reference

| Pattern | Where Used | Purpose |
| --- | --- | --- |
| Typestate | `domain/pipeline.rs` | Compile-time lifecycle enforcement |
| Phantom types | Pipeline stage markers | Zero-cost state tags |
| Interior mutability | `adapters/cache.rs`, `adapters/resilience.rs` | Shared mutable state behind `Arc` |
| Trait objects (`dyn`) | `ServiceRegistry`, `WorkerPool` | Runtime polymorphism for heterogeneous adapters |
| `async fn` in traits | All port traits | Native Rust 2024 async traits |
| `LazyLock` | `adapters/ai/claude.rs` | Safe static initialization |
| Newtype wrappers | `IdempotencyKey(ulid::Ulid)` | Type safety for primitive values |

---

## Extending Stygian

### Adding a new AI provider

1. Add `src/adapters/ai/my_provider.rs` implementing `AIProvider`.
2. Map its name in `AppConfig` or the CLI `--provider` flag.
3. Register it in `ExtractionService::new()`.

No changes to the domain or existing adapters required.

### Adding a new service kind

1. Implement `ScrapingService` in a new adapter file.
2. Register it via `ServiceRegistry::register("my-kind", Arc::new(MyAdapter::new()))`.
3. Reference it in TOML: `kind = "my-kind"`.

### GraphQL plugin

The `GraphQlTargetPlugin` port allows per-target GraphQL customization (auth headers, pagination
strategy) without modifying the generic `GraphQlAdapter`. Implement the trait and register via
`GraphQlPluginRegistry`.

---

## Testing strategy

| Layer | Approach |
| --- | --- |
| Domain | Pure unit tests — no I/O, no mocks needed |
| Ports | Compile-time tests (trait implementations in `tests/`) |
| Adapters | Integration tests with real endpoints (feature-gated) |
| Application | `NoopService` / `MockAIProvider` injected at runtime |

See `tests/integration.rs`, `tests/property_tests.rs`, and `tests/chaos.rs` for worked
examples.
