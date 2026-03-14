# Architecture

`stygian-graph` is built around the **Hexagonal Architecture** (Ports & Adapters) pattern.
The domain core is pure Rust with zero I/O dependencies. All external capabilities — HTTP, AI,
caching, queues — are declared as port traits and injected from the outside.

---

## Layer diagram

```text
┌──────────────────────────────────────────────────────┐
│                   CLI / Entry Points                 │
│              (src/bin/stygian.rs)                   │
├──────────────────────────────────────────────────────┤
│                  Application Layer                   │
│   ServiceRegistry · PipelineParser · Metrics         │
├──────────────────────────────────────────────────────┤
│                    Domain Core                       │
│       Pipeline · DagExecutor · WorkerPool            │
│       IdempotencyKey · PipelineTypestate             │
├──────────────────────────────────────────────────────┤
│                      Ports                           │
│  ScrapingService · AIProvider · CachePort · ...      │
├──────────────────────────────────────────────────────┤
│                    Adapters                          │
│   http · claude · openai · gemini · browser · ...    │
└──────────────────────────────────────────────────────┘
```

The dependency arrow always points **inward**:

```text
CLI → Application → Domain ← Ports ← Adapters
```

The domain never imports from adapters. Adapters implement port traits and are injected at
startup. This lets you swap any adapter — or mock it in tests — without touching business logic.

---

## Domain layer (`src/domain/`)

Pure Rust. Only `std`, `serde`, and arithmetic/pure-data crates allowed. No `tokio`, no
`reqwest`, no file I/O.

| Type | File | Purpose |
| --- | --- | --- |
| `Pipeline` | `domain/graph.rs` | Owned graph of `Node`s and `Edge`s; wraps a `petgraph` DAG |
| `DagExecutor` | `domain/graph.rs` | Topological sort → wave-based concurrent execution |
| `WorkerPool` | `domain/executor.rs` | Bounded Tokio worker pool with back-pressure |
| `IdempotencyKey` | `domain/idempotency.rs` | ULID-based key for safe retries |

### Pipeline typestate

Pipelines enforce their lifecycle at **compile time** using the typestate pattern:

```rust
use stygian_graph::domain::pipeline::{
    PipelineUnvalidated, PipelineValidated,
    PipelineExecuting, PipelineComplete,
};

let p: PipelineUnvalidated = PipelineUnvalidated::new("crawl", metadata);
let p: PipelineValidated   = p.validate()?;      // rejects invalid graphs here
let p: PipelineExecuting   = p.execute();         // only valid pipelines may execute
let p: PipelineComplete    = p.complete(result);  // only executing pipelines may complete
```

Out-of-order transitions are **compiler errors**. Phantom types carry zero runtime cost.

---

## Ports layer (`src/ports.rs`)

Port traits are the only interface the domain exposes to infrastructure. No adapter code ever
leaks inward.

```rust
/// Any scraping backend — HTTP, browser, Playwright, custom.
pub trait ScrapingService: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput>;
}

/// Any LLM — cloud APIs or local Ollama.
pub trait AIProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;
    async fn extract(&self, prompt: String, schema: Value) -> Result<Value>;
    async fn stream_extract(
        &self,
        prompt: String,
        schema: Value,
    ) -> Result<BoxStream<'static, Result<Value>>>;
}

/// Any cache backend — LRU, DashMap, Redis.
pub trait CachePort: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn set(&self, key: &str, value: String, ttl: Option<Duration>) -> Result<()>;
    async fn invalidate(&self, key: &str) -> Result<()>;
    async fn exists(&self, key: &str) -> Result<bool>;
}

/// Circuit breaker abstraction.
pub trait CircuitBreaker: Send + Sync {
    fn state(&self) -> CircuitState;
    fn record_success(&self);
    fn record_failure(&self);
    fn attempt_reset(&self) -> bool;
}

/// Token-bucket rate limiter abstraction.
pub trait RateLimiter: Send + Sync {
    async fn check_rate_limit(&self, key: &str) -> Result<bool>;
    async fn record_request(&self, key: &str) -> Result<()>;
}
```

---

## Adapters layer (`src/adapters/`)

Adapters implement port traits and handle real I/O. They are **never** imported by the domain.

| Adapter | Port | Notes |
| --- | --- | --- |
| `HttpAdapter` | `ScrapingService` | reqwest, UA rotation, cookie jar, retry |
| `BrowserAdapter` | `ScrapingService` | chromiumoxide via `stygian-browser` |
| `ClaudeProvider` | `AIProvider` | Anthropic API, streaming |
| `OpenAiProvider` | `AIProvider` | OpenAI Chat Completions API |
| `GeminiProvider` | `AIProvider` | Google Gemini API |
| `CopilotProvider` | `AIProvider` | GitHub Copilot API |
| `OllamaProvider` | `AIProvider` | Local LLM via Ollama HTTP API |
| `BoundedLruCache` | `CachePort` | LRU eviction, `NonZeroUsize` capacity limit |
| `DashMapCache` | `CachePort` | Concurrent hash map with TTL cleanup task |
| `CircuitBreakerImpl` | `CircuitBreaker` | Sliding-window failure threshold |
| `NoopCircuitBreaker` | `CircuitBreaker` | Passthrough — useful in tests |
| `NoopRateLimiter` | `RateLimiter` | Always allows — useful in tests |

### Adding a new adapter

1. Identify the port trait your adapter will implement (e.g. `ScrapingService`).
2. Create `src/adapters/my_adapter.rs`.
3. Implement the trait — use native `async fn` (Rust 2024, no `#[async_trait]` wrapper needed).
4. Re-export from `src/adapters/mod.rs`.
5. Register via `ServiceRegistry` at startup.

No changes to domain code are required.

---

## Application layer (`src/application/`)

Orchestrates adapters, holds runtime configuration, and owns the `ServiceRegistry`.

| Module | Role |
| --- | --- |
| `ServiceRegistry` | Runtime map of `name → Arc<dyn ScrapingService>` |
| `PipelineParser` | Parses JSON/TOML pipeline configs into `Pipeline` |
| `MetricsCollector` | Prometheus counter/histogram/gauge facade |
| `DagExecutor` (app) | Top-level entry point: parse → validate → execute → collect |
| `Config` | Environment-driven configuration with validation |

---

## DAG execution model

Pipeline execution proceeds in **topological waves**:

1. **Parse** — JSON/TOML config → `Pipeline` domain object.
2. **Validate** — check node uniqueness, edge validity, absence of cycles (Kahn's algorithm).
3. **Sort** — compute topological order; group into waves of independent nodes.
4. **Execute wave-by-wave** — all nodes in a wave run concurrently via `tokio::spawn`.
5. **Collect** — outputs from each wave become inputs to the next.

Wave-based execution maximises parallelism while respecting data dependencies.
