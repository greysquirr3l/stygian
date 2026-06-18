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
| **Health-check jitter** | Per-cycle random ±N% interval spread via `health_check_jitter_pct` — prevents thundering-herd against shared targets |
| **Circuit breaker** | Per-proxy lock-free FSM: `Closed → Open → HalfOpen`; auto-recovery after cooldown |
| **Capability filtering** | Tag proxies with `tls_profile`, `is_cdn_edge`, and arbitrary tags; filter at acquire time via `CapabilityRequirement` |
| **CDN-edge proxy type** | `ProxyType::CdnEdge` for CDN-fronted egress nodes alongside `Http`, `Https`, `Socks4`, `Socks5` |
| **Persistent connections** | `TransportPreference::PersistentTcp` with configurable max-requests and connection max-age |
| **In-memory pool** | No external database required; satisfies the `ProxyStoragePort` trait |
| **graph integration** | `ProxyManagerPort` trait for `stygian-graph` HTTP adapters (feature `graph`) |
| **browser integration** | Per-context proxy binding for `stygian-browser` (feature `browser`); TLS-profile-aware via `bind_proxy_with_tls_profile` |
| **SOCKS support** | `Socks4` and `Socks5` proxy types (feature `socks`) |
| **DNS TXT discovery** | `DnsTxtFetcher` resolves proxy lists from DNS TXT records (feature `dns-fetcher`) |
| **Adaptive intelligence** | Per-domain proxy scoring with exponential decay (default-on); explainable `ScoreReport` for observability |

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
  │ + jitter     │  │  (per proxy)    │  │
  └──────────────┘  └─────────────────┘  │
│                                         │
│  ┌──────────────────────────────────┐   │
│  │       RotationStrategy           │   │
│  │  RoundRobin / Random / Weighted  │   │
│  │  / LeastUsed                     │   │
│  └──────────────────────────────────┘   │
│                                         │
│  ┌──────────────────────────────────┐   │
│  │  CapabilityRequirement filter    │   │
│  │  tls_profile / cdn_edge / tags   │   │
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
| `intelligence_enabled` | `true` | Enable per-domain adaptive scoring |
| `intelligence_half_life` | 1 hour | Exponential decay applied to historical scores |
| `intelligence_weights` | success=0.6, challenge=0.3, latency=0.1 | Composite score weights |

---

## Adaptive intelligence (T86)

Per-domain proxy scoring layers on top of the rotation strategy. When the
manager has observed enough outcomes for a domain, candidates are ranked
by a composite score; otherwise the configured rotation strategy wins.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::types::ScoreWeights;

let storage = Arc::new(MemoryProxyStore::default());
let cfg = ProxyConfig {
    intelligence_weights: ScoreWeights {
        success: 0.7,
        challenge_penalty: 0.2,
        latency: 0.1,
    },
    ..ProxyConfig::default()
};
let mgr = ProxyManager::with_round_robin(storage, cfg)?;

// Record outcomes against the manager.
mgr.record_outcome("example.com", proxy_id, true, false, 120).await;

// Score-aware acquisition — falls back to rotation when no data exists.
let handle = mgr.acquire_for_domain_with_intelligence("example.com").await?;
handle.mark_success();

// Observability: explainable report of the chosen proxy + its score.
if let Some(report) = mgr.last_intelligence_report("example.com").await {
    tracing::info!(?report, "intelligence-scored selection");
}
```

See [`stygian-proxy::proxy_intelligence`](https://docs.rs/stygian-proxy/latest/stygian_proxy/proxy_intelligence/) for the `ProxyScore` /
`ScoreReport` / `SelectionBasis` types.
