# Rotation Strategies

`stygian-proxy` ships four built-in rotation strategies. All implement the
`RotationStrategy` trait and operate on a slice of `ProxyCandidate` values
built from the live pool. Strategies that find zero healthy candidates return
`ProxyError::AllProxiesUnhealthy` rather than panicking.

---

## Comparison

| Strategy | Best for | Notes |
| --- | --- | --- |
| `RoundRobinStrategy` | Even distribution across identical proxies | Atomic counter, lock-free |
| `RandomStrategy` | Spreading load unpredictably | `rand::rng()` per call; no shared state |
| `WeightedStrategy` | Prioritising faster or higher-quota proxies | Weighted random sampling; O(n) |
| `LeastUsedStrategy` | Never overloading a single proxy | Picks the candidate with the lowest total request count |

---

## RoundRobinStrategy

Distributes requests evenly across healthy proxies in insertion order. Uses an atomic
counter so there is no `Mutex` on the hot path.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_round_robin(storage, ProxyConfig::default()).unwrap();
```

`ProxyManager::with_round_robin` is the recommended default. The counter wraps safely
at `u64::MAX`, which at 1 million requests per second takes ~585,000 years.

---

## RandomStrategy

Picks a healthy proxy at random on every call. Useful when you want to avoid any
predictable rotation pattern that fingerprinting could detect.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::strategy::RandomStrategy;

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_strategy(
    storage,
    ProxyConfig::default(),
    Arc::new(RandomStrategy),
).unwrap();
```

---

## WeightedStrategy

Each `Proxy` has a `weight: u32` field (default `1`). `WeightedStrategy` performs weighted
random sampling so proxies with higher weights are selected proportionally more often.

```rust,no_run
use stygian_proxy::types::{Proxy, ProxyType};

// This proxy is 3× more likely to be selected than a weight-1 proxy.
let fast_proxy = Proxy {
    url: "http://fast.example.com:8080".into(),
    proxy_type: ProxyType::Http,
    weight: 3,
    ..Default::default()
};
```

Use this strategy when proxies have different capacities, quotas, or observed speeds.

---

## LeastUsedStrategy

Selects the healthy proxy with the **lowest total request count** at the time of the call.
This maximises even distribution over time even when proxies are added dynamically.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::strategy::LeastUsedStrategy;

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_strategy(
    storage,
    ProxyConfig::default(),
    Arc::new(LeastUsedStrategy),
).unwrap();
```

---

## Custom strategies

Implement `RotationStrategy` to plug in your own selection logic:

```rust,no_run
use async_trait::async_trait;
use stygian_proxy::error::ProxyResult;
use stygian_proxy::strategy::{ProxyCandidate, RotationStrategy};

/// Always pick the proxy with the best success rate.
pub struct BestSuccessRateStrategy;

#[async_trait]
impl RotationStrategy for BestSuccessRateStrategy {
    async fn select<'a>(
        &self,
        candidates: &'a [ProxyCandidate],
    ) -> ProxyResult<&'a ProxyCandidate> {
        candidates
            .iter()
            .filter(|c| c.healthy)
            .max_by(|a, b| {
                let ra = a.metrics.success_rate();
                let rb = b.metrics.success_rate();
                ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or(stygian_proxy::error::ProxyError::AllProxiesUnhealthy)
    }
}
```

Pass it to `ProxyManager::with_strategy(storage, config, Arc::new(BestSuccessRateStrategy))`.

---

## ProxyCandidate fields

| Field | Type | Description |
| --- | --- | --- |
| `id` | `Uuid` | Stable proxy identifier |
| `weight` | `u32` | Relative selection weight |
| `metrics` | `Arc<ProxyMetrics>` | Shared atomics: requests, failures, latency |
| `healthy` | `bool` | Result of the last health check |

`ProxyMetrics` exposes `success_rate() -> f64` and `avg_latency_ms() -> f64` computed from
the atomic counters without any locking.
