# Observability

`mycelium-graph` exposes structured metrics and distributed tracing out of the box.
Both are opt-in: neither requires code changes to your adapters or domain logic.

---

## Prometheus metrics

Enable the `metrics` feature flag:

```toml
mycelium-graph = { version = "0.1", features = ["metrics"] }
```

### Creating a collector

```rust
use mycelium_graph::application::MetricsCollector;

let metrics = MetricsCollector::new();
```

`MetricsCollector` registers counters, histograms, and gauges on the global Prometheus
registry automatically. It is `Clone + Send + Sync` and safe to share across threads.

### Exposing `/metrics`

Attach the Prometheus scrape handler to any HTTP server. Example with Axum:

```rust
use axum::{Router, routing::get};
use mycelium_graph::application::MetricsCollector;

let metrics  = MetricsCollector::new();
let handler  = metrics.prometheus_handler();

let app = Router::new()
    .route("/metrics", get(handler))
    .route("/health",  get(|| async { "ok" }));

axum::serve(listener, app).await?;
```

### Available metrics

| Metric name | Type | Labels | Description |
|---|---|---|---|
| `mycelium_requests_total` | counter | `service`, `status` | Total requests per adapter |
| `mycelium_request_duration_seconds` | histogram | `service` | Request latency distribution |
| `mycelium_errors_total` | counter | `service`, `error_kind` | Errors by type |
| `mycelium_worker_pool_active` | gauge | `pool` | Active workers |
| `mycelium_worker_pool_queued` | gauge | `pool` | Queued tasks |
| `mycelium_circuit_breaker_state` | gauge | `service` | 0=closed, 1=open, 2=half-open |
| `mycelium_cache_hits_total` | counter | `cache` | Cache hits |
| `mycelium_cache_misses_total` | counter | `cache` | Cache misses |

---

## Structured tracing

`mycelium-graph` instruments all hot paths with the [`tracing`](https://docs.rs/tracing) crate.
Any compatible subscriber (JSON, OTLP, Jaeger) receives full span trees.

### Basic JSON logging

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

tracing_subscriber::registry()
    .with(EnvFilter::from_default_env()
        .add_directive("mycelium_graph=debug".parse()?)
        .add_directive("mycelium_browser=info".parse()?))
    .with(tracing_subscriber::fmt::layer().json())
    .init();
```

Set `RUST_LOG=mycelium_graph=trace` at runtime for full span output.

### OpenTelemetry export (Jaeger / OTLP)

```toml
[dependencies]
opentelemetry          = "0.22"
opentelemetry-otlp     = { version = "0.15", features = ["grpc-tonic"] }
tracing-opentelemetry  = "0.23"
```

```rust
use opentelemetry_otlp::WithExportConfig;
use tracing_opentelemetry::OpenTelemetryLayer;

let tracer = opentelemetry_otlp::new_pipeline()
    .tracing()
    .with_exporter(
        opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint("http://localhost:4317"),
    )
    .install_batch(opentelemetry::runtime::Tokio)?;

tracing_subscriber::registry()
    .with(EnvFilter::from_default_env())
    .with(OpenTelemetryLayer::new(tracer))
    .init();
```

### Key spans

| Span | Attributes | Emitted by |
|---|---|---|
| `dag_execute` | `pipeline_id`, `node_count`, `wave_count` | `DagExecutor` |
| `wave_execute` | `wave`, `node_ids[]` | `DagExecutor` |
| `service_call` | `service`, `url` | `ServiceRegistry` |
| `ai_extract` | `provider`, `model`, `tokens_in`, `tokens_out` | AI adapters |
| `cache_lookup` | `hit`, `key_prefix` | Cache adapters |
| `circuit_breaker` | `service`, `state_transition` | `CircuitBreakerImpl` |

---

## Health checks

`MetricsCollector` exposes a health-check endpoint that reports the state of every
registered service:

```rust
let health_json = metrics.health_check(&registry).await;
// {"status":"ok","services":{"http":"healthy","ai_claude":"healthy"}}
```

A service is reported as `"degraded"` when its circuit breaker is half-open, and
`"unhealthy"` when it is open.
