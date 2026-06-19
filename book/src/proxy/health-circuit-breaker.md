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

### Interval jitter

To prevent all health-check probes from firing at the same instant (a
"thundering herd" against shared infrastructure), set `health_check_jitter_pct`
in `ProxyConfig`. Each sleep between cycles is perturbed by a random factor in
`[1 − pct, 1 + pct)` using a thread-local CSPRNG.

```rust,no_run
use std::time::Duration;
use stygian_proxy::ProxyConfig;

let config = ProxyConfig {
    health_check_interval: Duration::from_secs(60),
    // Each cycle sleeps between 48 s and 72 s (±20%)
    health_check_jitter_pct: 0.20,
    ..ProxyConfig::default()
};
```

`health_check_jitter_pct` is clamped to `[0.0, 0.99]`. Set it to `0.0`
(the default) for a fixed interval.

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
| Shared datacenter egress | Set `health_check_jitter_pct = 0.20–0.30` to spread probes across the interval window |
| Dev / testing | `health_check_interval = 5 s`; `health_check_url` pointing to a local echo server |

---

## Persistent-connection config

For workloads that benefit from reusing TCP connections across multiple requests
(e.g. CONNECT-tunnelled HTTP/1.1 or HTTP/2), set `TransportPreference::PersistentTcp`
at the routing layer and configure the lifetime bounds:

```rust,no_run
use stygian_proxy::ProxyConfig;

let config = ProxyConfig {
    // Retire a connection after 200 requests or 5 minutes, whichever comes first.
    max_requests_per_connection: Some(200),
    connection_max_age_secs: Some(300),
    ..ProxyConfig::default()
};
```

| Field | Default | Description |
| --- | --- | --- |
| `max_requests_per_connection` | `None` | Maximum requests before connection retirement |
| `connection_max_age_secs` | `None` | Wall-clock lifetime cap in seconds |

When both fields are `None` the connection lifetime is governed entirely by the proxy
server and TCP keepalive. Set either limit to prevent silent connection staleness.

---

## Thompson-sampling interaction

When the `bayesian-rotation` cargo feature is enabled and the manager is
built with [`ProxyManager::with_thompson_sampling`](https://docs.rs/stygian-proxy/0.14/stygian_proxy/struct.ProxyManager.html#method.with_thompson_sampling),
`ThompsonStrategy` is registered as **both** the rotation strategy and
the Bayesian observer. The same `ProxyHandle::mark_success` and
drop-failure signals that drive the circuit breaker also feed the
per-proxy `Beta(α, β)` posterior.

| Signal | Effect on circuit breaker | Effect on Thompson posterior |
| --- | --- | --- |
| `handle.mark_success()` | Resets failure counter, closes circuit | `α += 1` on the bound proxy |
| Drop without `mark_success()` | Increments failure counter | `β += 1` on the bound proxy |
| Health-check success | Closes circuit | `α += 1` (if the proxy was already bound to a session, also feeds the strategy) |
| Health-check failure | Opens circuit after threshold | `β += 1` |

There is **no separate observer call** required at the call site. The
two observation streams (live traffic + background health checks) feed
the same posterior and the same circuit breaker state.

To seed the bandit from a known-good feed at startup (warm-up before
the cold-start traffic reaches statistical equilibrium), call
`strategy_warmup_observe(proxy_id, success)` once for each entry in your
trust list. See the [Thompson-sampling Bayesian rotation](strategies.md#thompsonsampling-bayesian-rotation)
section for the full API.

---

## TLS-profiled request mode

When the `tls-profiled` cargo feature is enabled, `ProxyConfig` exposes
a `tls_profiled_request_mode: ProfiledRequestMode` field that the
`HealthChecker` consults at construction time to choose how probes
exercise the TLS stack. The default is `ProfiledRequestMode::Disabled`,
which produces the same probes as the un-profiled `HealthChecker`.

Other variants exercise the proxy's TLS profile end-to-end so a broken
profile trips the circuit before any live request hits it. The exact
variant catalogue is in the rustdoc; the relevant operational effect is
that **a profile mismatch surfaces as a circuit-open event**, not as a
runtime TLS error in the request path.
