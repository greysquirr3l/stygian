# Custom Adapters

This guide walks through building a production-quality custom adapter from scratch.
The worked example implements a hypothetical `PlaywrightService` — a browser automation
adapter that drives the Playwright protocol instead of CDP.

By the end you will know how to:

- Implement any port trait as a new adapter
- Register the adapter in the service registry
- Wrap the adapter in resilience primitives
- Write integration tests against it

---

## Prerequisites

Read [Architecture](./architecture.md) first. Adapters live in `src/adapters/` and implement
traits defined in `src/ports.rs`. The domain never imports adapters — only ports.

---

## Step 1: Choose the right port

| Port | Use when |
| --- | --- |
| `ScrapingService` | Fetching or processing content in a new way |
| `AIProvider` | Adding a new LLM or language model API |
| `CachePort` | Adding a new cache backend (Redis, Memcached, …) |
| `SigningPort` | Attaching signatures, HMAC tokens, or authentication material to outgoing requests |

`PlaywrightService` fetches rendered HTML, so it implements `ScrapingService`.

---

## Step 2: Scaffold the adapter

Create `src/adapters/playwright.rs`:

```rust
//! Playwright browser adapter.

use std::time::Duration;
use serde_json::Value;

use crate::domain::error::{StygianError, ServiceError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Configuration for the Playwright adapter.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::playwright::PlaywrightConfig;
///
/// let cfg = PlaywrightConfig {
///     ws_endpoint:     "ws://localhost:3000/playwright".into(),
///     default_timeout: std::time::Duration::from_secs(30),
///     headless:        true,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct PlaywrightConfig {
    pub ws_endpoint:     String,
    pub default_timeout: Duration,
    pub headless:        bool,
}

impl Default for PlaywrightConfig {
    fn default() -> Self {
        Self {
            ws_endpoint:     "ws://localhost:3000/playwright".into(),
            default_timeout: Duration::from_secs(30),
            headless:        true,
        }
    }
}

// ── Adapter ───────────────────────────────────────────────────────────────────

/// Browser adapter backed by a Playwright JSON-RPC server.
pub struct PlaywrightService {
    config: PlaywrightConfig,
}

impl PlaywrightService {
    pub fn new(config: PlaywrightConfig) -> Self {
        Self { config }
    }
}

// ── Port implementation ───────────────────────────────────────────────────────

impl ScrapingService for PlaywrightService {
    fn name(&self) -> &'static str {
        "playwright"
    }

    /// Navigate to `input.url`, wait for network idle, return rendered HTML.
    ///
    /// # Errors
    ///
    /// `ServiceError::Unavailable` — Playwright server is unreachable.  
    /// `ServiceError::Timeout`     — navigation exceeded `default_timeout`.
    async fn execute(&self, input: ServiceInput) -> crate::domain::error::Result<ServiceOutput> {
        // Real implementation would:
        //   1. Connect to self.config.ws_endpoint via WebSocket
        //   2. Send Browser.newPage  
        //   3. Navigate to input.url with self.config.default_timeout
        //   4. Await "networkidle" lifecycle event
        //   5. Call Page.getContent() for rendered HTML
        //   6. Close the page
        Err(StygianError::Service(ServiceError::Unavailable(
            format!("PlaywrightService not connected (url={})", input.url),
        )))
    }
}
```

---

## Step 3: Re-export

Add a `pub mod playwright;` line to `src/adapters/mod.rs`:

```rust
// src/adapters/mod.rs
pub mod http;
pub mod browser;
pub mod claude;
// ... existing adapters ...
pub mod playwright;   // ← add this line
```

---

## Step 4: Register in the service registry

In your binary entry point or `application/executor.rs` startup code:

```rust
use std::sync::Arc;
use stygian_graph::adapters::playwright::{PlaywrightConfig, PlaywrightService};
use stygian_graph::application::registry::ServiceRegistry;

let registry = ServiceRegistry::new();

let config = PlaywrightConfig {
    ws_endpoint: std::env::var("PLAYWRIGHT_WS_ENDPOINT")
        .unwrap_or_else(|_| "ws://localhost:3000/playwright".into()),
    ..Default::default()
};

registry.register(
    "playwright".into(),
    Arc::new(PlaywrightService::new(config)),
);
```

Pipelines can now reference the adapter with `service = "playwright"` in any node.

---

## Step 5: Add resilience wrappers

Wrap before registering to get circuit-breaker and retry behaviour for free:

```rust
use std::sync::Arc;
use std::time::Duration;
use stygian_graph::adapters::resilience::{CircuitBreakerImpl, RetryPolicy, ResilientAdapter};

let cb = CircuitBreakerImpl::new(
    5,                          // open after 5 consecutive failures
    Duration::from_secs(120),   // half-open probe after 2 min
);

let policy = RetryPolicy::exponential(
    3,                          // max 3 attempts
    Duration::from_millis(200), // initial back-off
    Duration::from_secs(5),     // cap
);

let resilient = ResilientAdapter::new(
    Arc::new(PlaywrightService::new(config)),
    Arc::new(cb),
    policy,
);

registry.register("playwright".into(), Arc::new(resilient));
```

---

## Step 6: Integration tests

Use stygian's built-in mock transport for unit tests that don't require a real server:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use stygian_graph::ports::ServiceInput;

    fn make_input(url: &str) -> ServiceInput {
        ServiceInput { url: url.into(), config: serde_json::Value::Null, ..Default::default() }
    }

    #[tokio::test]
    async fn returns_unavailable_when_not_connected() {
        let svc = PlaywrightService::new(PlaywrightConfig::default());
        let err = svc.execute(make_input("https://example.com")).await.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }
}
```

For full integration tests that require a real Playwright server, mark them with
`#[ignore = "requires playwright server"]` so CI passes without the dependency.

---

## Adapter checklist

Before merging a new adapter:

- [ ] Implements the correct port trait
- [ ] `name()` returns a lowercase, kebab-case identifier
- [ ] All public types have doc comments with an example
- [ ] `Default` impl provided where sensible
- [ ] Config can be loaded from environment variables
- [ ] At least one unit test that does not require external services
- [ ] Integration tests marked `#[ignore = "requires …"]`
- [ ] Re-exported from `src/adapters/mod.rs`
- [ ] Registered in the service registry example in the binary
