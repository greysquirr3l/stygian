# Health Checking & Circuit Breaker

`stygian-proxy` keeps the pool fresh through two complementary mechanisms: an
**async health checker** that periodically probes each proxy, and a **per-proxy
circuit breaker** that trips automatically when a proxy starts failing live
requests.

---

## Health checker

`HealthChecker` runs a background `tokio` task that probes every registered
proxy on a configurable interval. Probes run concurrently via `JoinSet` so a
slow or timing-out proxy does not delay checks for healthy ones.

### Starting the health checker

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let config = ProxyConfig {
    health_check_url: "https://httpbin.org/ip".into(),
    health_check_interval: std::time::Duration::from_secs(60),
    health_check_timeout: std::time::Duration::from_secs(5),
    ..ProxyConfig::default()
};
let manager = Arc::new(
    ProxyManager::with_round_robin(storage, config)?
);

let (cancel, _task) = manager.start();

// When shutting down:
cancel.cancel();
```

### On-demand check

Call `health_checker.check_once().await` to run a single probe cycle without
spawning a background task. Useful in tests or before the first request batch.

### Health state

Each proxy's health state is stored in `HealthMap` — an `Arc<DashMap<Uuid, bool>>`.
The rotation strategy reads this map when building the `ProxyCandidate` slice; proxies
marked unhealthy are filtered out before selection.

---

## Circuit breaker

Every proxy gets its own `CircuitBreaker` when it is added to the pool. The
circuit breaker is a **lock-free atomic FSM** with three states:

```
          failure ≥ threshold
  CLOSED ──────────────────────► OPEN
    ▲                               │
    │  success on probe             │ half_open_after elapsed
    │                               ▼
    └──────────────────────── HALF-OPEN
          success on probe
```

| State | Behaviour |
| --- | --- |
| **Closed** | Proxy is selectable; failures increment counter |
| **Open** | Proxy is excluded from selection; requests are not attempted |
| **Half-Open** | One probe attempt allowed; success → Closed, failure → Open (timer reset) |

### How it integrates with ProxyHandle

`acquire_proxy()` returns a `ProxyHandle`. The handle holds an `Arc<CircuitBreaker>`.

- **`handle.mark_success()`** — resets the failure counter, moves the circuit to Closed.
- **Drop without `mark_success`** — records a failure; opens the circuit after `circuit_open_threshold` consecutive failures.

```rust,no_run
let handle = manager.acquire_proxy().await?;

match do_request(&handle.proxy_url).await {
    Ok(_) => handle.mark_success(),
    Err(e) => {
        // handle is dropped here → failure recorded automatically
        eprintln!("request failed: {e}");
    }
}
```

### ProxyHandle::direct()

For code paths that conditionally use a proxy, `ProxyHandle::direct()` returns a
sentinel handle with an empty URL and a noop circuit breaker that can never trip.
Pass it wherever a `ProxyHandle` is expected when no proxy should be used.

```rust,no_run
use stygian_proxy::manager::ProxyHandle;

let handle = if use_proxy {
    manager.acquire_proxy().await?
} else {
    ProxyHandle::direct()
};
```

---

## PoolStats

`manager.pool_stats().await` returns a `PoolStats` snapshot:

```rust,no_run
let stats = manager.pool_stats().await?;
println!("total={} healthy={} open_circuits={}",
    stats.total, stats.healthy, stats.open);
```

| Field | Description |
| --- | --- |
| `total` | Total proxies registered |
| `healthy` | Proxies that passed the last health check |
| `open` | Proxies whose circuit breaker is currently Open |

---

## Tuning recommendations

| Scenario | Recommendation |
| --- | --- |
| High-churn scraping (many short requests) | Lower `circuit_open_threshold` to 2–3; shorter `circuit_half_open_after` (10–15 s) |
| Long-lived connections | Raise `circuit_open_threshold` to 10+; extend `circuit_half_open_after` (60–120 s) |
| Residential proxies (naturally flaky) | Use `WeightedStrategy` with lower weights for known flaky proxies; keep threshold at default (5) |
| Dev / testing | `health_check_interval = 5 s`; `health_check_url` pointing to a local echo server |
