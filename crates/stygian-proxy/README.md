# stygian-proxy

High-performance, resilient proxy rotation for the Stygian scraping ecosystem.

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](../../LICENSE)
[![Crates.io](https://img.shields.io/crates/v/stygian-proxy.svg)](https://crates.io/crates/stygian-proxy)

---

## Features

| Feature | Description | Default |
| --------- | ------------- | --------- |
| Rotation strategies | Round-robin, random, weighted, least-used | ✓ |
| Circuit breakers | Per-proxy failure tracking & recovery | ✓ |
| Health checking | Background async health prober | ✓ |
| RAII proxy handles | Automatic success/failure recording on drop | ✓ |
| `socks` | SOCKS4/5 proxy support via reqwest | — |
| `graph` | Integration with stygian-graph HTTP adapters | — |
| `browser` | Integration with stygian-browser pool (`ProxyManagerBridge` + `ProxyLeaseAdapter`) | — |
| `tls-profiled` | TLS fingerprint profiling for proxy connections (requires `browser` feature) | — |
| `mcp` | MCP (Model Context Protocol) tools | — |

---

## Features (Core Capabilities)

---

## Installation

```toml
[dependencies]
stygian-proxy = "*"
tokio = { version = "1", features = ["full"] }
```

Enable optional features:

```toml
# SOCKS4/5 proxy support
stygian-proxy = { version = "*", features = ["socks"] }

# Integration with stygian-graph HTTP adapters
stygian-proxy = { version = "*", features = ["graph"] }

# Integration with stygian-browser pool
stygian-proxy = { version = "*", features = ["browser"] }
```

---

## Quick Start

```rust,no_run
use stygian_proxy::{ProxyManager, MemoryProxyStore, Proxy, types::{ProxyType, ProxyConfig}};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build a pool with round-robin rotation
    let manager = ProxyManager::with_round_robin(Arc::new(MemoryProxyStore::default()), ProxyConfig::default())?;

    // Add proxies
    manager.add_proxy(Proxy {
        url: "http://proxy1.example.com:8080".into(),
        proxy_type: ProxyType::Http,
        username: None,
        password: None,
    }).await?;

    manager.add_proxy(Proxy {
        url: "http://proxy2.example.com:8080".into(),
        proxy_type: ProxyType::Http,
        username: Some("user".into()),
        password: Some("pass".into()),
    }).await?;

    // Start the background health checker
    let (cancel, _task) = manager.start();

    // Acquire a proxy — skips any with an open circuit breaker
    let handle = manager.acquire_proxy().await?;
    println!("Using proxy: {}", handle.proxy_url);

    // Mark success to keep the circuit breaker closed
    handle.mark_success();

    // Pool stats
    let stats = manager.pool_stats().await?;
    println!("Pool: {}/{} healthy, {} open", stats.healthy, stats.total, stats.open);

    cancel.cancel();
    Ok(())
}
```

---

## Rotation Strategies

| Strategy | Constructor | Behaviour |
| --------- | ----------- | --------- |
| `RoundRobinStrategy` | `ProxyManager::with_round_robin` | Cycles through healthy proxies in order |
| `RandomStrategy` | `ProxyManager::with_random` | Picks a healthy proxy at random each time |
| `WeightedStrategy` | `ProxyManager::with_weighted` | Selects proportionally to each proxy's `weight` field |
| `LeastUsedStrategy` | `ProxyManager::with_least_used` | Prefers the proxy with the lowest total request count |

Custom strategies implement `RotationStrategy`:

```rust,no_run
use stygian_proxy::strategy::{RotationStrategy, ProxyCandidate};
use stygian_proxy::error::ProxyResult;
use async_trait::async_trait;

#[derive(Debug)]
struct MyStrategy;

#[async_trait]
impl RotationStrategy for MyStrategy {
    async fn select<'a>(&self, candidates: &'a [ProxyCandidate]) -> ProxyResult<&'a ProxyCandidate> {
        // pick the candidate with the best success rate
        candidates.iter().max_by(|a, b| {
            a.metrics.success_rate().partial_cmp(&b.metrics.success_rate())
                .unwrap_or(std::cmp::Ordering::Equal)
        }).ok_or(stygian_proxy::error::ProxyError::AllProxiesUnhealthy)
    }
}
```

---

## Circuit Breaker

Each proxy has its own `CircuitBreaker`. After `circuit_open_threshold` consecutive failures the breaker opens, and the proxy is excluded from rotation for `circuit_half_open_after`. After that window the proxy is tried once in HalfOpen state — a success closes it; another failure reopens it.

```rust,no_run
use stygian_proxy::{ProxyManager, MemoryProxyStore, types::ProxyConfig};
use std::sync::Arc;
use std::time::Duration;

let config = ProxyConfig {
    // Open after 5 consecutive failures
    circuit_open_threshold: 5,
    // Try again after 60 seconds
    circuit_half_open_after: Duration::from_secs(60),
    ..Default::default()
};

let manager = ProxyManager::with_round_robin(Arc::new(MemoryProxyStore::default()), config)?;
```

If a `ProxyHandle` is dropped without calling `mark_success()`, the circuit breaker records a failure automatically.

---

## Health Checking

`ProxyManager::start()` spawns a background task that probes each proxy on a configurable interval and updates per-proxy health scores:

```rust,no_run
use stygian_proxy::{ProxyManager, MemoryProxyStore, types::ProxyConfig};
use std::sync::Arc;
use std::time::Duration;

let config = ProxyConfig {
    health_check_interval: Duration::from_secs(30),
    health_check_timeout: Duration::from_secs(5),
    ..Default::default()
};

let manager = ProxyManager::with_round_robin(Arc::new(MemoryProxyStore::default()), config)?;
let (cancel_token, health_task) = manager.start();

// Graceful shutdown
cancel_token.cancel();
```

---

## stygian-graph Integration

With the `graph` feature, the pool implements `ProxyManagerPort` so stygian-graph adapters can rotate proxies per-request:

```toml
stygian-proxy = { version = "*", features = ["graph"] }
stygian-graph = "*"
```

```rust,no_run
use stygian_proxy::{ProxyManager, MemoryProxyStore, graph::ProxyManagerPort};
use stygian_proxy::types::ProxyConfig;
use std::sync::Arc;

let manager = ProxyManager::with_round_robin(Arc::new(MemoryProxyStore::default()), ProxyConfig::default())?;
// Pass as Arc<dyn ProxyManagerPort> to RestApiAdapter or HttpAdapter
```

---

## stygian-browser Integration

With the `browser` feature, `ProxyManagerBridge` implements [`stygian_browser::proxy::ProxySource`] so
stygian-browser contexts can acquire live pool proxies at launch time:

```toml
stygian-proxy = { version = "*", features = ["browser"] }
stygian-browser = "*"
```

```rust,no_run
use stygian_proxy::{ProxyManager, MemoryProxyStore, browser::ProxyManagerBridge};
use stygian_proxy::types::ProxyConfig;
use stygian_browser::BrowserConfig;
use std::sync::Arc;

let manager = Arc::new(
    ProxyManager::with_round_robin(Arc::new(MemoryProxyStore::default()), ProxyConfig::default())?
);

let bridge = Arc::new(ProxyManagerBridge::new(manager));

// Pass the bridge to browser config
let config = BrowserConfig::builder()
    .proxy_source(bridge)
    .build();

// Each browser context acquires its own proxy from the pool
// On release: proxy success/failure automatically recorded to circuit breaker
```

Under the hood:

- `ProxyManagerBridge` implements `ProxySource` (browser's port trait)
- Each acquired proxy is wrapped in `ProxyLeaseAdapter`
- When `mark_success()` is called on the lease, it updates the proxy's circuit breaker
- On drop without marked success, the proxy is recorded as failed

---

## License

`AGPL-3.0-only OR LicenseRef-Commercial` — see [LICENSE](../../LICENSE) and [LICENSE-COMMERCIAL.md](../../LICENSE-COMMERCIAL.md).
