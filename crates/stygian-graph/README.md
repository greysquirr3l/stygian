# stygian-graph

High-performance, graph-based web scraping engine treating pipelines as DAGs with pluggable service modules.

[![Crates.io](https://img.shields.io/crates/v/stygian-graph.svg)](https://crates.io/crates/stygian-graph)
[![Documentation](https://img.shields.io/badge/docs-github.io-blue)](https://greysquirr3l.github.io/stygian)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](../../LICENSE)
[![Tests](https://img.shields.io/badge/tests-209%20passing-brightgreen)](https://github.com/greysquirr3l/stygian/actions)
[![Coverage](https://img.shields.io/badge/coverage-~72%25-yellowgreen)](https://github.com/greysquirr3l/stygian/actions)

---

## Features

| Feature | Description |
| --------- | ------------- |
| **Hexagonal architecture** | Domain core isolated from infrastructure concerns |
| **Graph execution** | DAG-based pipeline with topological sort, wave-by-wave execution |
| **Pluggable adapters** | HTTP, browser, AI providers, storage — add custom services easily |
| **AI extraction** | Claude, GPT, Gemini, GitHub Copilot, Ollama — structured data from HTML |
| **Multi-modal** | Images, PDFs, videos via vision APIs |
| **Distributed execution** | Redis/Valkey work queues for horizontal scaling |
| **Circuit breaker** | Degradation when services fail (browser → HTTP fallback) |
| **Idempotency** | Safe retries with deduplication keys |
| **Observability** | Prometheus metrics, structured tracing |

---

## Installation

```toml
[dependencies]
stygian-graph = "0.1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

Enable optional features:

```toml
stygian-graph = { version = "0.1", features = ["browser", "ai-claude", "distributed"] }
```

---

## Quick Start

### Basic Scraping Pipeline

```rust
use stygian_graph::{Pipeline, PipelineBuilder};
use stygian_graph::adapters::{HttpAdapter, NoopAdapter};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define pipeline as JSON config
    let config = json!({
        "nodes": [
            {"id": "fetch", "service": "http"},
            {"id": "extract", "service": "noop"}
        ],
        "edges": [
            {"from": "fetch", "to": "extract"}
        ]
    });

    let pipeline = Pipeline::from_config(config)?;
    
    let input = json!({"url": "https://example.com"});
    let results = pipeline.execute(input).await?;
    
    println!("Extracted: {:?}", results);
    Ok(())
}
```

### With Browser Rendering

```rust
use stygian_graph::adapters::{BrowserAdapter, BrowserAdapterConfig};
use std::time::Duration;

let config = BrowserAdapterConfig {
    timeout: Duration::from_secs(30),
    headless: true,
    default_stealth: StealthLevel::Advanced,
    ..Default::default()
};

let browser_adapter = BrowserAdapter::with_config(config);
```

### With AI Extraction

```rust
use stygian_graph::adapters::{ClaudeAdapter, ClaudeConfig};

let config = ClaudeConfig {
    api_key: std::env::var("ANTHROPIC_API_KEY")?,
    model: "claude-3-5-sonnet-20241022".to_string(),
    max_tokens: 4096,
    ..Default::default()
};

let ai = ClaudeAdapter::with_config(config);
```

---

## Architecture

### Hexagonal (Ports & Adapters)

```
┌─────────────────────────────────────────────┐
│           Application Layer                 │
│  (DagExecutor, ServiceRegistry, Metrics)    │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│            Port Traits                      │
│  (ScrapingService, AiProvider, WorkQueue)   │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│              Adapters                       │
│  HTTP │ Browser │ Claude │ Redis │ ...      │
└─────────────────────────────────────────────┘
```

### Domain Rules

- **Zero I/O in domain** — all external interactions through ports
- **Dependency inversion** — adapters depend on ports, never vice versa
- **Typestate pattern** — compile-time pipeline validation
- **Zero-cost abstractions** — generics over Arc/Box where possible

---

## Pipeline Configuration

Define scraping flows as JSON:

```json
{
  "nodes": [
    {
      "id": "fetch_html",
      "service": "http",
      "config": {
        "timeout_ms": 10000,
        "user_agent": "Mozilla/5.0..."
      }
    },
    {
      "id": "render_js",
      "service": "browser",
      "config": {
        "wait_strategy": "network_idle",
        "stealth_level": "advanced"
      }
    },
    {
      "id": "extract_data",
      "service": "ai_claude",
      "config": {
        "model": "claude-3-5-sonnet-20241022",
        "schema": {
          "title": "string",
          "price": "number",
          "availability": "boolean"
        }
      }
    }
  ],
  "edges": [
    {"from": "fetch_html", "to": "render_js"},
    {"from": "render_js", "to": "extract_data"}
  ]
}
```

### Validation

Pipelines are validated before execution:

1. **Node integrity** — IDs unique, services registered
2. **Edge validity** — all edges connect existing nodes
3. **Cycle detection** — Kahn's topological sort
4. **Reachability** — all nodes connected in single DAG

---

## Adapters

### HTTP Adapter

```rust
use stygian_graph::adapters::{HttpAdapter, HttpConfig};

let config = HttpConfig {
    timeout: Duration::from_secs(10),
    user_agent: Some("MyBot/1.0".to_string()),
    follow_redirects: true,
    max_redirects: 5,
};

let adapter = HttpAdapter::with_config(config);
```

### Browser Adapter

Requires `browser` feature + `stygian-browser` crate:

```rust
use stygian_graph::adapters::{BrowserAdapter, BrowserAdapterConfig};

let adapter = BrowserAdapter::with_config(BrowserAdapterConfig {
    headless: true,
    viewport_width: 1920,
    viewport_height: 1080,
    ..Default::default()
});
```

### AI Adapters

**Claude** (Anthropic):

```rust
use stygian_graph::adapters::ClaudeAdapter;

let adapter = ClaudeAdapter::new(
    std::env::var("ANTHROPIC_API_KEY")?,
    "claude-3-5-sonnet-20241022"
);
```

**OpenAI**:

```rust
use stygian_graph::adapters::OpenAiAdapter;

let adapter = OpenAiAdapter::new(
    std::env::var("OPENAI_API_KEY")?,
    "gpt-4o"
);
```

**Gemini** (Google):

```rust
use stygian_graph::adapters::GeminiAdapter;

let adapter = GeminiAdapter::new(
    std::env::var("GOOGLE_API_KEY")?,
    "gemini-2.0-flash"
);
```

---

## Distributed Execution

Use Redis/Valkey for work queue backend:

```rust
use stygian_graph::adapters::{DistributedDagExecutor, RedisWorkQueue};

let queue = RedisWorkQueue::new("redis://localhost:6379").await?;
let executor = DistributedDagExecutor::new(queue, 10); // 10 workers

let results = executor.execute_wave("pipeline-1", tasks, &services).await?;
```

---

## Observability

### Prometheus Metrics

```rust
use stygian_graph::application::MetricsCollector;

let metrics = MetricsCollector::new();
let prometheus_handler = metrics.prometheus_handler();

// Expose on /metrics endpoint
axum::Router::new()
    .route("/metrics", axum::routing::get(prometheus_handler))
```

### Structured Tracing

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

tracing_subscriber::registry()
    .with(tracing_subscriber::EnvFilter::new(
        std::env::var("RUST_LOG").unwrap_or_else(|_| "stygian_graph=info".into())
    ))
    .with(tracing_subscriber::fmt::layer().json())
    .init();
```

---

## Testing

```bash
# Unit tests
cargo test --lib

# Integration tests
cargo test --test integration

# All features (browser integration tests require Chrome)
cargo test --all-features

# Benchmarks
cargo bench

# Measure coverage (requires cargo-tarpaulin)
cargo tarpaulin -p stygian-graph --all-features --ignore-tests --out Lcov
```

**Coverage**: ~72% line coverage across 209 workspace tests. Key modules at or near 100%:
`config`, `executor`, `idempotency`, `service_registry`, and all AI adapter unit tests.
Adapters requiring live external services (HTTP, browser) are tested with mock ports.

---

## Performance

- **Concurrency**: Tokio for I/O, Rayon for CPU-bound
- **Zero-copy**: `Arc<str>` for shared strings
- **Lock-free**: DashMap for concurrent access
- **Pool reuse**: HTTP clients, browser instances

Benchmarks (Apple M4 Pro):

- DAG executor: ~50µs overhead per wave
- HTTP adapter: ~2ms per request (cached DNS)
- Browser adapter: <100ms acquisition (warm pool)

---

## Examples

See [examples/](../../examples) for complete pipelines:

- `basic-scrape.toml` — Simple HTTP → parse flow
- `javascript-rendering.toml` — Browser-based extraction
- `multi-provider.toml` — AI fallback chain
- `distributed.toml` — Redis work queue setup

---

## License

Licensed under the [GNU Affero General Public License v3.0](../../LICENSE) (`AGPL-3.0-only`).

---

## Links

- [Documentation](https://docs.rs/stygian-graph)
- [Repository](https://github.com/greysquirr3l/stygian)
- [Crates.io](https://crates.io/crates/stygian-graph)
- [Issues](https://github.com/greysquirr3l/stygian/issues)
