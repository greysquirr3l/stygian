# Proxy Rotation Overview

`stygian-proxy` is a high-performance, resilient proxy rotation library for the Stygian
scraping ecosystem. It manages a pool of proxy endpoints, tracks per-proxy health and
latency metrics, and integrates directly with both `stygian-graph` HTTP adapters and
`stygian-browser` page contexts.

---

## Feature summary

| Feature | Description |
| --- | --- |
| **Rotation strategies** | Round-robin, random, weighted (by proxy weight), least-used (by request count) |
| **Per-proxy metrics** | Atomic latency and success-rate tracking — zero lock contention |
| **Async health checker** | Configurable-interval background task; each proxy probed concurrently via `JoinSet` |
| **Circuit breaker** | Per-proxy lock-free FSM: `Closed → Open → HalfOpen`; auto-recovery after cooldown |
| **In-memory pool** | No external database required; satisfies the `ProxyStoragePort` trait |
| **graph integration** | `ProxyManagerPort` trait for `stygian-graph` HTTP adapters (feature `graph`) |
| **browser integration** | Per-context proxy binding for `stygian-browser` (feature `browser`) |
| **SOCKS support** | `Socks4` and `Socks5` proxy types (feature `socks`) |

---

## Quick start

Add the dependency:

```toml
[dependencies]
stygian-proxy = { version = "*", features = ["graph"] }
```

Build a pool and make a request:

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::types::{Proxy, ProxyType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage = Arc::new(MemoryProxyStore::default());
    let manager = Arc::new(
        ProxyManager::with_round_robin(storage, ProxyConfig::default())?
    );

    // Register proxies at startup (or dynamically at any time)
    manager.add_proxy(Proxy {
        url: "http://proxy1.example.com:8080".into(),
        proxy_type: ProxyType::Http,
        username: None,
        password: None,
        weight: 1,
        tags: vec!["us-east".into()],
    }).await?;

    // Start background health checks
    let (cancel, _task) = manager.start();

    // Acquire a proxy for a request
    let handle = manager.acquire_proxy().await?;
    println!("using proxy: {}", handle.proxy_url);

    // Signal success — omitting this counts as a failure toward the circuit breaker
    handle.mark_success();

    cancel.cancel(); // stop health checker
    Ok(())
}
```

---

## Architecture

```
┌─────────────────────────────────────────┐
│              ProxyManager               │
│                                         │
│  ┌──────────────┐  ┌─────────────────┐  │
│  │ HealthChecker│  │ CircuitBreakers │  │
│  │  (background)│  │  (per proxy)    │  │
│  └──────────────┘  └─────────────────┘  │
│                                         │
│  ┌──────────────────────────────────┐   │
│  │       RotationStrategy           │   │
│  │  RoundRobin / Random / Weighted  │   │
│  │  / LeastUsed                     │   │
│  └──────────────────────────────────┘   │
│                                         │
│  ┌──────────────────────────────────┐   │
│  │       ProxyStoragePort           │   │
│  │  MemoryProxyStore (built-in)     │   │
│  └──────────────────────────────────┘   │
└─────────────────────────────────────────┘
         │                    │
         ▼                    ▼
  stygian-graph        stygian-browser
  HTTP adapters        page contexts
```

`ProxyManager` is the main entry point. It composes a storage backend, a rotation strategy,
a background health checker, and a map of per-proxy circuit breakers. Callers interact only
with `acquire_proxy()` → `ProxyHandle` → `mark_success()`.

---

## Cargo features

| Feature | Enables |
| --- | --- |
| *(default: none)* | Core pool, strategies, health checker, circuit breaker |
| `graph` | `ProxyManagerPort` trait + blanket impl + `NoopProxyManager` |
| `browser` | `BrowserProxySource` trait + `ProxyManagerBridge` |
| `socks` | `ProxyType::Socks4` and `ProxyType::Socks5` variants |

---

## ProxyConfig defaults

| Field | Default | Description |
| --- | --- | --- |
| `health_check_url` | `https://httpbin.org/ip` | URL probed to verify liveness |
| `health_check_interval` | 60 s | How often to run checks |
| `health_check_timeout` | 5 s | Per-probe HTTP timeout |
| `circuit_open_threshold` | 5 | Consecutive failures before circuit opens |
| `circuit_half_open_after` | 30 s | Cooldown before attempting recovery |
