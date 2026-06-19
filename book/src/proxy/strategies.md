# Rotation Strategies

`stygian-proxy` ships five built-in rotation strategies. Four — `RoundRobin`,
`Random`, `Weighted`, `LeastUsed` — are always compiled. A fifth,
`ThompsonSampling`, is gated behind the `bayesian-rotation` cargo feature.
All implement the `RotationStrategy` trait and operate on a slice of
`ProxyCandidate` values built from the live pool. Strategies that find zero
healthy candidates return `ProxyError::AllProxiesUnhealthy` rather than
panicking.

---

## Comparison

| Strategy | Best for | Notes |
| --- | --- | --- |
| `RoundRobinStrategy` | Even distribution across identical proxies | Atomic counter, lock-free |
| `RandomStrategy` | Spreading load unpredictably | `rand::rng()` per call; no shared state |
| `WeightedStrategy` | Prioritising faster or higher-quota proxies | Weighted random sampling; O(n) |
| `LeastUsedStrategy` | Never overloading a single proxy | Picks the candidate with the lowest total request count |
| `ThompsonSampling` *(feature `bayesian-rotation`)* | Concentrating traffic on proxies that actually work against the target | Per-proxy `Beta(α, β)` posterior with `AtomicU64` counters; 76% vs 36% round-robin in the internal `ProxyOps` benchmark (549k requests / 7 days) |

---

## RoundRobinStrategy

Distributes requests evenly across healthy proxies in insertion order. Uses an atomic
counter so there is no `Mutex` on the hot path.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;
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
let manager = ProxyManager::builder()
    .storage(storage)
    .strategy(Arc::new(RandomStrategy))
    .config(ProxyConfig::default())
    .build()?;
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
let manager = ProxyManager::builder()
    .storage(storage)
    .strategy(Arc::new(LeastUsedStrategy))
    .config(ProxyConfig::default())
    .build()?;
```

---

## ThompsonSampling (Bayesian rotation)

`ThompsonSampling` keeps a per-proxy `Beta(α, β)` posterior over the success
rate, with `AtomicU64` counters updated by `ProxyHandle::mark_success` and
the implicit "failure on drop" path. On every acquire it samples each
candidate's posterior and picks the proxy with the highest Thompson draw —
proxies with stronger evidence of success get more traffic, and the strategy
**concentrates** rather than **distributes** load.

The internal `ProxyOps` benchmark (549,114 requests / 7 days, identical
proxies) cites **76% success rate** vs **36% for round-robin** on protected
targets. Hot-path acquire stays sub-microsecond.

```rust,no_run
use std::sync::Arc;
use std::time::Duration;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_thompson_sampling(
    storage,
    ProxyConfig::default(),
    Duration::from_secs(300), // decay_interval — defaults to 5 min
)?;

// Optional: seed the bandit from a known-good feed so cold-start
// traffic is already informed.
mgr.strategy_warmup_observe(proxy_id_a, true).await;
mgr.strategy_warmup_observe(proxy_id_b, false).await;
```

Two knobs control how quickly stale observations age out of the posterior:

- `decay_interval` (default 5 min) — how often to apply the decay.
- `decay_factor` (default 0.95) — multiplicative weight applied to old observations.

A prior-bias seam lets the strategy weight proxies whose
`TargetVendorCompatibility` is higher for the target vendor. See the
`ThompsonStrategy` rustdoc for the full surface.

When the strategy is enabled, `mark_success` and the drop-failure path both
feed the bandit — no separate observer call is required at the call site.

---

## Capability filtering

All strategies operate on a pre-filtered `ProxyCandidate` slice. Before a
strategy runs, `ProxyManager` filters the pool by `CapabilityRequirement`.
Use `acquire_with_capabilities(&req)` to express requirements at call time:

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::types::{CapabilityRequirement, IpClassRequirement, VendorId, well_known};

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;

let req = CapabilityRequirement {
    target_vendor: Some(VendorId::Akamai),
    require_ip_class: Some(IpClassRequirement::at_least(
        stygian_proxy::types::IpClass::Residential,
    )),
    require_tls_profile: Some("chrome-131".into()),
    require_asn: Some(well_known::KNOWN_ASN_AKAMAI),  // 20_940
    require_city: Some("Cambridge".into()),
    ..Default::default()
};
let handle = manager.acquire_with_capabilities(&req).await?;
```

When no candidate in the pool satisfies the requirement, the call returns
`ProxyError::NoCompatibleProxy` — distinct from
`ProxyError::AllProxiesUnhealthy`, which signals every proxy is currently
circuit-open rather than structurally incompatible.

### ProxyCapabilities fields

`ProxyCapabilities` is the per-proxy capability record. Every field is
`#[serde(default)]`, so a serialised payload from an older 0.13.x release
deserialises cleanly into a `ProxyCapabilities::default()`.

| Field | Type | Description |
| --- | --- | --- |
| `supports_https_connect` | `bool` | Proxy supports `CONNECT` for HTTPS tunnelling |
| `supports_socks5_udp` | `bool` | Proxy supports SOCKS5 with UDP relay |
| `supports_http3_tunnel` | `bool` | Proxy supports HTTP/3 (QUIC) tunnelling |
| `geo_country` | `Option<String>` | ISO-3166-1 alpha-2 country code for the egress IP |
| `geo_confidence` | `Option<f32>` | Confidence score `[0.0, 1.0]` for the geo data; `None` if unstated |
| `is_cdn_edge` | `bool` | `true` for CDN-fronted egress nodes (`ProxyType::CdnEdge`) |
| `cdn_provider` | `Option<String>` | Advisory CDN provider name (e.g. `"cloudflare"`) |
| `tls_profile` | `Option<String>` | Named TLS fingerprint profile (`"chrome-131"`, `"firefox-120"`, `"curl"`) |
| `ip_class` | `IpClass` | Trust class — `Mobile` / `Isp` / `Residential` / `Datacenter` / `Unknown` (default) |
| `target_compatibility` | `TargetVendorCompatibility` | Per-vendor trust tier overrides |
| `asn` | `Option<u32>` | Egress AS number; `None` means provider did not tag it |
| `city` | `Option<String>` | Egress city (operator-declared) |
| `postal_code` | `Option<String>` | Egress postal/ZIP code (operator-declared) |

`Proxy` itself exposes two top-level fields that mirror and can override the
nested values: `ip_class: IpClass` and
`target_compatibility: TargetVendorCompatibility`.

### CapabilityRequirement fields

`CapabilityRequirement` is the call-site filter. Every field is independently
`#[serde(default, skip_serializing_if = "Option::is_none")]` — a serialised
requirement with no filters round-trips to `CapabilityRequirement::default()`.

| Field | Type | Semantics |
| --- | --- | --- |
| `require_https_connect` | `bool` | Must be `true` on the proxy's `capabilities.supports_https_connect` |
| `require_socks5_udp` | `bool` | Must be `true` on `supports_socks5_udp` |
| `require_http3_tunnel` | `bool` | Must be `true` on `supports_http3_tunnel` |
| `require_geo_country` | `Option<String>` | Exact match against `geo_country` |
| `require_cdn_edge` | `bool` | Proxy must advertise `is_cdn_edge = true` |
| `require_tls_profile` | `Option<String>` | Exact match against `tls_profile` |
| `require_ip_class` | `Option<IpClassRequirement>` | Minimum trust class (rank-based — see below) |
| `target_vendor` | `Option<VendorId>` | Proxy's `target_compatibility` for this vendor must be non-`Blocked` |
| `require_asn` | `Option<u32>` | Exact match against `capabilities.asn` |
| `require_city` | `Option<String>` | Exact match against `capabilities.city` |
| `require_postal_code` | `Option<String>` | Exact match against `capabilities.postal_code` |

`IpClassRequirement` wraps a `minimum: IpClass` field. The check is
rank-based: `Mobile` (rank 4) outranks `Isp` (3) which outranks
`Residential` (2) which outranks `Datacenter` (1); `Unknown` (0) satisfies
only an `at_least(Unknown)` requirement. A proxy tagged `IpClass::Unknown`
never satisfies a non-empty IP-class requirement, which is the safe default
for legacy un-tagged proxies.

The `target_vendor` filter is the fail-secure gate on free-list pools: a
proxy whose `target_compatibility.get(VendorId) == Some(TrustTier::Blocked)`
does not satisfy `target_vendor = Some(VendorId)`. Free-list fetchers
populate every ingested proxy with `TargetVendorCompatibility::default_blocked()`
so operators cannot accidentally route premium traffic through a public
free-list pool.

### Annotating proxies at registration time

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::types::{
    IpClass, Proxy, ProxyCapabilities, ProxyType, TargetVendorCompatibility,
    TrustTier, VendorId, well_known,
};

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;

// Operator-curated residential proxy, known to defeat Akamai.
manager.add_proxy(Proxy {
    url: "http://user:pass@edge1.example.com:8080".into(),
    proxy_type: ProxyType::Http,
    weight: 1,
    tags: vec!["eu-west".into()],
    capabilities: ProxyCapabilities {
        supports_https_connect: true,
        is_cdn_edge: true,
        cdn_provider: Some("akamai".into()),
        tls_profile: Some("chrome-131".into()),
        asn: Some(well_known::KNOWN_ASN_AKAMAI),
        city: Some("Cambridge".into()),
        ..Default::default()
    },
    ip_class: IpClass::Residential,
    target_compatibility: TargetVendorCompatibility::default()
        .set(VendorId::Akamai, TrustTier::Preferred),
    ..Default::default()
}).await?;
```

For ingest-time metadata without going through a `Proxy` struct literal, see
[`add_proxy_with_metadata`](https://docs.rs/stygian-proxy/0.14/stygian_proxy/struct.ProxyManager.html#method.add_proxy_with_metadata),
which validates the URL against `vendor_quirks::check` and accepts
`(url, asn, city, postal_code)` directly:

```rust,no_run
use stygian_proxy::types::well_known;
manager.add_proxy_with_metadata(
    "http://user:pass@edge1.example.com:8080".into(),
    well_known::KNOWN_ASN_AKAMAI,
    "Cambridge".into(),
    "02142".into(),
).await?;
```

---

## Custom strategies

Implement `RotationStrategy` to plug in your own selection logic. Rust 2024
supports `async fn` in traits natively — no `async_trait` macro needed.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::error::ProxyResult;
use stygian_proxy::strategy::{ProxyCandidate, RotationStrategy};

/// Always pick the proxy with the best success rate.
pub struct BestSuccessRateStrategy;

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

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::builder()
    .storage(storage)
    .strategy(Arc::new(BestSuccessRateStrategy))
    .config(ProxyConfig::default())
    .build()?;
```

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
