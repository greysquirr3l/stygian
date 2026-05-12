# Resilience & Fallback Chains

Stygian's graph engine provides two complementary primitives for production-grade
resilience: the **CircuitBreaker** and the **FallbackChainService**.  Together they
let you build self-healing scraping pipelines that degrade gracefully under failure
rather than propagating errors upstream.

---

## Circuit breaker

`CircuitBreakerImpl` lives in `stygian-graph` and implements the classic three-state
machine:

```
Closed  →  threshold failures reached  →  Open
Open    →  reset_timeout elapsed        →  Half-Open (probe)
Half-Open  →  probe success  →  Closed
Half-Open  →  probe failure  →  Open
```

### Configuration

| Field | Default | Description |
| ----- | ------- | ----------- |
| `failure_threshold` | 5 | Consecutive failures before circuit opens |
| `reset_timeout` | 60 s | Time before the breaker enters Half-Open |

Convenience constructors:

```rust
use stygian_graph::adapters::fallback::{
    default_primary_breaker,   // threshold 5, reset 60 s
    default_fallback_breaker,  // threshold 3, reset 120 s
};
```

### Querying state

```rust
use stygian_graph::adapters::resilience::CircuitBreakerState;

match breaker.state() {
    CircuitBreakerState::Closed    => { /* normal path */ }
    CircuitBreakerState::Open      => { /* skip, fail fast */ }
    CircuitBreakerState::HalfOpen  => { /* probe once */ }
}
```

---

## Fallback chain

`FallbackChainService` wraps an ordered list of `(Arc<dyn ScrapingService>, CircuitBreakerImpl)`
pairs.  On each call it iterates from first to last, skipping any entry whose circuit
is `Open`, and returns the first successful result.  If every service fails it returns
the last error.

### Builder API

```rust
use std::sync::Arc;
use stygian_graph::adapters::fallback::{
    FallbackChainService, default_fallback_breaker, default_primary_breaker,
};
use stygian_graph::adapters::http::{HttpAdapter, HttpConfig};
use stygian_graph::adapters::noop::NoopService;

let chain = FallbackChainService::builder()
    .add(Arc::new(HttpAdapter::with_config(HttpConfig::default())),
         default_primary_breaker())
    .add(Arc::new(NoopService),
         default_fallback_breaker())
    .named("my-chain")
    .build();
```

Methods on `FallbackChainService`:

| Method | Description |
| ------ | ----------- |
| `execute(input)` | Run the chain; returns first success or last error |
| `name()` | Chain name set via `.named(…)` |
| `len()` | Number of entries |
| `is_empty()` | True if no entries |

### HTTP → Plugin fallback (real-world example)

The `stygian-mcp` aggregator wires the following chain automatically:

```rust
use stygian_graph::adapters::fallback::{
    FallbackChainService, default_fallback_breaker, default_primary_breaker,
};
use stygian_graph::adapters::http::{HttpAdapter, HttpConfig};
use stygian_plugin::adapters::{ExtractionEngine, PluginExtractionAdapter};
use stygian_plugin::storage::{FileTemplateStore, MemoryIdempotencyStore};
use std::sync::Arc;

let template_store   = Arc::new(FileTemplateStore::new("./plugin-templates".into()));
let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

let chain = Arc::new(
    FallbackChainService::builder()
        .add(Arc::new(HttpAdapter::with_config(HttpConfig::default())),
             default_primary_breaker())
        .add(Arc::new(PluginExtractionAdapter::new(
                Arc::clone(&template_store),
                Arc::new(ExtractionEngine),
                Arc::clone(&idempotency_store),
             )),
             default_fallback_breaker())
        .named("http-to-plugin")
        .build(),
);
```

Both `McpPluginServer` (for template CRUD) and `PluginExtractionAdapter` (for fallback
extraction) share the **same `Arc<FileTemplateStore>`**, so templates created over MCP
are immediately available in the fallback path without any cache invalidation.

---

## Execution flow

```
execute(ServiceInput { url, template_id, idempotency_key })
  │
  ├─ entry[0]: circuit Closed
  │    ├─ call service[0]
  │    │    ├─ Ok(output) → record success → return output ✓
  │    │    └─ Err(e)    → record failure → try next
  │    │
  ├─ entry[1]: circuit Closed
  │    ├─ call service[1]
  │    │    ├─ Ok(output) → record success → return output ✓
  │    │    └─ Err(e)    → record failure → (no more entries)
  │
  └─ return last error ✗
```

An `Open` circuit is always skipped without calling the underlying service.  The reset
probe (Half-Open) is the only exception: one call is allowed through; success closes
the circuit, failure re-opens it.

---

## Idempotency

Pass an `idempotency_key` in `ServiceInput` to deduplicate retries.  The
`PluginExtractionAdapter` checks `MemoryIdempotencyStore` before executing and
records the result after.  Repeated calls with the same key return the cached result
without re-running extraction.

---

## Testing

Unit tests for `FallbackChainService` live in `crates/stygian-graph/src/adapters/fallback.rs`.
Cross-crate integration tests live in `crates/stygian-plugin/tests/fallback_chain_integration.rs`
and cover:

- Primary succeeds → fallback never called
- Primary fails → fallback fires and returns result
- Circuit opens after threshold → primary skipped entirely
- Shared store contract: templates saved through `McpPluginServer` are visible in `PluginExtractionAdapter`
- Empty chain returns `ServiceError::Unavailable`
- Idempotent fallback calls return the same result for identical keys
