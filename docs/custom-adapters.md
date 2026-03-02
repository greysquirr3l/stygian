# Building Custom Adapters for Stygian

This guide walks through implementing a production-quality custom adapter from scratch.
We'll build a hypothetical `PlaywrightService` — a browser automation adapter that uses the
Playwright protocol — as a complete worked example.

By the end you will know how to:
- Implement any port trait as a new adapter
- Register the adapter in the service registry
- Wrap the adapter in resilience primitives
- Write integration tests against your adapter

---

## Prerequisites

Read [architecture.md](./architecture.md) first to understand the Ports & Adapters model.
Adapters live in `src/adapters/` and implement traits defined in `src/ports.rs`.

---

## Step 1: Choose the right port trait

Stygian has three primary port traits you can implement:

| Port | Use when |
| --- | --- |
| `ScrapingService` | You're adding a new way to fetch or process content |
| `AIProvider` | You're adding a new LLM or language model API |
| `CachePort` | You're adding a new cache backend (Redis, Memcached, …) |

Our `PlaywrightService` fetches rendered HTML, so it implements `ScrapingService`.

---

## Step 2: Scaffold the adapter file

Create `src/adapters/playwright.rs`:

```rust
//! Playwright browser adapter.
//!
//! Drives a Playwright-compatible browser via its JSON-RPC protocol to capture
//! fully-rendered HTML from JavaScript-heavy pages.

use std::time::Duration;
use async_trait::async_trait;
use serde_json::json;

use crate::domain::error::{StygianError, ServiceError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Configuration ────────────────────────────────────────────────────────────

/// Configuration for the Playwright adapter.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::playwright::PlaywrightConfig;
/// use std::time::Duration;
///
/// let config = PlaywrightConfig {
///     ws_endpoint: "ws://localhost:3000/playwright".into(),
///     default_timeout: Duration::from_secs(30),
///     headless: true,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct PlaywrightConfig {
    /// WebSocket endpoint of the running Playwright server process
    pub ws_endpoint: String,
    /// Default navigation timeout
    pub default_timeout: Duration,
    /// Run headlessly (no visible window)
    pub headless: bool,
}

impl Default for PlaywrightConfig {
    fn default() -> Self {
        Self {
            ws_endpoint: "ws://localhost:3000/playwright".into(),
            default_timeout: Duration::from_secs(30),
            headless: true,
        }
    }
}

// ─── Adapter struct ───────────────────────────────────────────────────────────

/// Browser adapter backed by a Playwright server.
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use stygian_graph::adapters::playwright::{PlaywrightService, PlaywrightConfig};
///
/// let svc = PlaywrightService::new(PlaywrightConfig::default());
/// let svc: Arc<dyn stygian_graph::ports::ScrapingService> = Arc::new(svc);
/// ```
pub struct PlaywrightService {
    config: PlaywrightConfig,
}

impl PlaywrightService {
    /// Create a new `PlaywrightService` with the given configuration.
    pub fn new(config: PlaywrightConfig) -> Self {
        Self { config }
    }
}

// ─── Port implementation ──────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for PlaywrightService {
    /// Returns the unique service identifier referenced in TOML pipelines.
    fn name(&self) -> &'static str {
        "playwright"
    }

    /// Navigate to `input.url`, wait for network idle, and return rendered HTML.
    ///
    /// # Errors
    ///
    /// Returns `ServiceError::Unavailable` if the Playwright server is
    /// unreachable, or `ServiceError::Timeout` if navigation exceeds
    /// `config.default_timeout`.
    async fn execute(
        &self,
        input: ServiceInput,
    ) -> crate::domain::error::Result<ServiceOutput> {
        // In a real implementation you would:
        //   1. Connect to self.config.ws_endpoint via WebSocket
        //   2. Send a Browser.newPage command
        //   3. Navigate to input.url with the configured timeout
        //   4. Wait for "networkidle" lifecycle event
        //   5. Call Page.getContent() to retrieve the rendered HTML
        //   6. Close the page
        //
        // Here we return a placeholder to demonstrate the shape of the code.
        let _ = &self.config; // suppress unused warning in example
        Err(StygianError::Service(ServiceError::Unavailable(
            format!("PlaywrightService not connected (url={})", input.url),
        )))
    }
}
```

---

## Step 3: Re-export from `adapters.rs`

```rust
// src/adapters.rs  (add to the existing pub mod list)
pub mod playwright;
```

---

## Step 4: Register in the service registry

Wire it up in the application startup code (`application/executor.rs` or your binary entry point):

```rust
use std::sync::Arc;
use stygian_graph::adapters::playwright::{PlaywrightConfig, PlaywrightService};
use stygian_graph::application::registry::ServiceRegistry;

let registry = ServiceRegistry::new();

let pw_config = PlaywrightConfig {
    ws_endpoint: std::env::var("PLAYWRIGHT_WS_ENDPOINT")
        .unwrap_or_else(|_| "ws://localhost:3000/playwright".into()),
    ..Default::default()
};
registry.register(
    "playwright".into(),
    Arc::new(PlaywrightService::new(pw_config)),
);
```

Now pipelines can reference the adapter with `service = "playwright"` in any `[[nodes]]` block.

---

## Step 5: Add resilience wrappers

Wrap your adapter with the built-in resilience primitives before registering:

```rust
use std::sync::Arc;
use std::time::Duration;
use stygian_graph::adapters::resilience::{CircuitBreakerImpl, RetryPolicy};
use stygian_graph::ports::CircuitBreaker;

// Circuit breaker: open after 5 consecutive failures, try to reset after 2 min
let cb = Arc::new(CircuitBreakerImpl::new(5, Duration::from_secs(120)));

// Retry policy: exponential backoff, max 3 attempts, 200ms–5s window
let retry = RetryPolicy::new(
    3,
    Duration::from_millis(200),
    Duration::from_secs(5),
).with_jitter_ms(50);

registry.register(
    "playwright".into(),
    Arc::new(PlaywrightService::new(pw_config)),
);
// The registry and DAG executor check the circuit state before dispatching.
```

---

## Step 6: Use interior mutability for adapter state

If your adapter needs mutable internal state (connection pool, session storage, counter), use `tokio::sync::Mutex` or `std::sync::RwLock` — **not `&mut self`**:

```rust
use tokio::sync::Mutex;

pub struct PlaywrightService {
    config: PlaywrightConfig,
    connection: Mutex<Option<PlaywrightConnection>>,   // lazily initialised
    request_count: std::sync::atomic::AtomicU64,       // cheap atomic counter
}

impl PlaywrightService {
    async fn ensure_connected(&self) -> Result<(), StygianError> {
        let mut conn = self.connection.lock().await;
        if conn.is_none() {
            *conn = Some(PlaywrightConnection::connect(&self.config).await?);
        }
        Ok(())
    }
}
```

`ScrapingService::execute` takes `&self` (shared reference), so all mutation must go through interior mutability.

---

## Step 7: Connection pooling

For high-throughput scenarios, use a pool of browser pages instead of creating one per request:

```rust
use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct PlaywrightService {
    config: PlaywrightConfig,
    // Allow at most `pool_size` concurrent page navigations
    semaphore: Arc<Semaphore>,
}

impl PlaywrightService {
    pub fn new(config: PlaywrightConfig, pool_size: usize) -> Self {
        Self {
            config,
            semaphore: Arc::new(Semaphore::new(pool_size)),
        }
    }
}

// In execute():
let _permit = self.semaphore
    .acquire()
    .await
    .map_err(|_| ServiceError::Unavailable("semaphore closed".into()))?;
// … do navigation …
// permit drops automatically when this scope ends, releasing the slot
```

---

## Step 8: Write integration tests

Use the `NoopService` pattern from `tests/integration.rs` as a baseline, then add trait-level tests:

```rust
// tests/playwright_integration.rs  (feature-gated to avoid CI failures)
#[cfg(feature = "playwright-integration")]
#[tokio::test]
async fn playwright_renders_javascript_spa() {
    let svc = PlaywrightService::new(PlaywrightConfig::default());
    let input = ServiceInput {
        url: "https://angular.io".into(),
        params: serde_json::json!({}),
    };

    let output = svc.execute(input).await.expect("playwright execute");
    assert!(output.data.contains("Getting started"), "SPA content missing");
}
```

For unit tests without a real browser, implement a `MockPlaywrightService`:

```rust
struct MockPlaywrightService {
    response: String,
}

#[async_trait]
impl ScrapingService for MockPlaywrightService {
    fn name(&self) -> &'static str { "playwright-mock" }

    async fn execute(&self, _input: ServiceInput) -> Result<ServiceOutput> {
        Ok(ServiceOutput {
            data: self.response.clone(),
            metadata: serde_json::json!({"source": "mock"}),
        })
    }
}
```

---

## Step 9: Document your adapter

Every public type and method must have a doc comment with an example (enforced by `#![warn(missing_docs)]`):

```rust
/// Connects to a Playwright server and returns rendered HTML.
///
/// # Example
///
/// ```rust,no_run
/// # use stygian_graph::adapters::playwright::{PlaywrightService, PlaywrightConfig};
/// # use stygian_graph::ports::{ScrapingService, ServiceInput};
/// # use serde_json::json;
/// # tokio_test::block_on(async {
/// let svc = PlaywrightService::new(PlaywrightConfig::default());
/// let result = svc.execute(ServiceInput {
///     url: "https://example.com".into(),
///     params: json!({}),
/// }).await;
/// # });
/// ```
fn name(&self) -> &'static str { "playwright" }
```

---

## Checklist

Before submitting your adapter:

- [ ] Implements the correct port trait with `#[async_trait]`
- [ ] `fn name() -> &'static str` returns a stable, lowercase identifier
- [ ] `execute()` has no `.unwrap()` or `.expect()` calls
- [ ] All errors use `ServiceError` or `StygianError` — no `anyhow`
- [ ] Interior mutability used for all shared mutable state
- [ ] Doc comment with example on every public item
- [ ] Re-exported from `src/adapters.rs`
- [ ] Registered in the `ServiceRegistry` at startup
- [ ] At least one unit test (mock) and one compile-time trait test
- [ ] `cargo clippy --workspace -- -D warnings` passes
