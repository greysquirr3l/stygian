# Ecosystem Integrations

`stygian-proxy` can be wired into both `stygian-graph` HTTP adapters and
`stygian-browser` page contexts via optional Cargo features.

---

## stygian-graph (`graph` feature)

Enable the `graph` feature to get `ProxyManagerPort` — the trait that decouples
graph HTTP adapters from any specific proxy implementation.

```toml
[dependencies]
stygian-proxy = { version = "*", features = ["graph"] }
```

### ProxyManagerPort

```rust,no_run
use stygian_proxy::graph::ProxyManagerPort;

// ProxyManager already implements ProxyManagerPort via a blanket impl.
// Inside any HTTP adapter:
async fn fetch(proxy_src: &dyn ProxyManagerPort, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let handle = proxy_src.acquire_proxy().await?;
    // ... make request through handle.proxy_url ...
    handle.mark_success();
    Ok(String::new())
}
```

### BoxedProxyManager

`BoxedProxyManager` is a type alias for `Arc<dyn ProxyManagerPort>`. Use it to
store a proxy source in `HttpAdapter` or any other adapter without naming the
concrete type:

```rust,no_run
use std::sync::Arc;
use stygian_proxy::graph::{BoxedProxyManager, ProxyManagerPort};
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let manager = Arc::new(
    ProxyManager::with_round_robin(storage, ProxyConfig::default()).unwrap()
);

// Coerce to the trait object
let boxed: BoxedProxyManager = manager;
```

### NoopProxyManager

When no proxying is needed, pass `NoopProxyManager` instead. It returns
`ProxyHandle::direct()` on every call — a noop handle with an empty URL.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::graph::{BoxedProxyManager, NoopProxyManager};

let no_proxy: BoxedProxyManager = Arc::new(NoopProxyManager);
```

This avoids `Option<BoxedProxyManager>` branches in adapter code: always pass a
`BoxedProxyManager`, just swap implementations at construction time.

---

## stygian-browser (`browser` feature)

Enable the `browser` feature to bind a specific proxy to each browser page context.

```toml
[dependencies]
stygian-proxy = { version = "*", features = ["browser"] }
```

### ProxyManagerBridge

`ProxyManagerBridge` wraps a `ProxyManager` and exposes `bind_proxy()`, which
acquires one proxy and returns `(proxy_url, ProxyHandle)`. The URL can be passed
directly to `chromiumoxide` when launching a browser context.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::browser::ProxyManagerBridge;

let storage = Arc::new(MemoryProxyStore::default());
let manager = Arc::new(
    ProxyManager::with_round_robin(storage, ProxyConfig::default()).unwrap()
);
let bridge = ProxyManagerBridge::new(Arc::clone(&manager));

// In your browser context setup:
let (proxy_url, handle) = bridge.bind_proxy().await?;
// Pass proxy_url to chromiumoxide BrowserConfig::builder().proxy(proxy_url)
// ...
handle.mark_success(); // after the page session completes
```

### BrowserProxySource

`BrowserProxySource` is the trait implemented by `ProxyManagerBridge`. Implement
it directly to plug in any proxy source without depending on `ProxyManager`:

```rust,no_run
use async_trait::async_trait;
use stygian_proxy::browser::BrowserProxySource;
use stygian_proxy::manager::ProxyHandle;
use stygian_proxy::error::ProxyResult;

pub struct MyProxySource;

#[async_trait]
impl BrowserProxySource for MyProxySource {
    async fn bind_proxy(&self) -> ProxyResult<(String, ProxyHandle)> {
        // return (proxy_url, handle)
        Ok((
            "http://my-proxy.example.com:8080".into(),
            ProxyHandle::direct(),
        ))
    }
}
```

---

## DNS TXT proxy discovery (`dns-fetcher` feature)

Enable the `dns-fetcher` feature to resolve proxy lists from DNS TXT records.
This is useful for infrastructure-managed proxy registries that publish endpoints
via DNS rather than an HTTP API.

```toml
[dependencies]
stygian-proxy = { version = "*", features = ["dns-fetcher"] }
```

`DnsTxtFetcher` implements `ProxyFetcher` and queries a DNS TXT record where each
string is a proxy URL (`http://host:port` or `socks5://host:port`):

```rust,no_run
use stygian_proxy::DnsTxtFetcher;
use stygian_proxy::fetcher::{ProxyFetcher, load_from_fetcher};
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage = Arc::new(MemoryProxyStore::default());
    let manager = Arc::new(
        ProxyManager::with_round_robin(Arc::clone(&storage), ProxyConfig::default())?
    );

    // Load proxies from DNS TXT at "proxies.internal.example.com"
    let fetcher = DnsTxtFetcher::new("proxies.internal.example.com");
    let loaded = load_from_fetcher(&fetcher, &manager).await?;
    println!("loaded {loaded} proxies from DNS");

    Ok(())
}
```

The TXT record format is one proxy URL per string value:

```
proxies.internal.example.com. 60 IN TXT "http://10.0.1.5:8080"
proxies.internal.example.com. 60 IN TXT "socks5://10.0.1.6:1080"
```

`DnsTxtFetcher` uses `hickory-resolver` under the hood and currently uses the
system resolver configuration by default.

For hardened deployments, prefer constraining the lookup zone and timeout:

```rust,no_run
use std::time::Duration;
use stygian_proxy::DnsTxtFetcher;

let fetcher = DnsTxtFetcher::new("proxies.internal.example.com")
    .with_allowed_zone_suffixes(vec!["internal.example.com".to_string()])
    .with_lookup_timeout(Duration::from_secs(3));
```

This helps reduce risk from misconfigured DNS sources by limiting discovery to
trusted suffixes and bounding lookup latency.

---

## TLS-profile-aware browser binding

`ProxyManagerBridge` also exposes `bind_proxy_with_tls_profile()`, which
combines proxy acquisition with a `CapabilityRequirement` that matches a
specific TLS fingerprint profile. This ensures the acquired proxy is
capable of presenting the correct TLS fingerprint for the browser session.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::browser::ProxyManagerBridge;

let bridge = ProxyManagerBridge::new(Arc::new(
    ProxyManager::with_round_robin(
        Arc::new(MemoryProxyStore::default()),
        ProxyConfig::default(),
    ).unwrap()
));

// Acquire a proxy that carries the "chrome_131" TLS profile
let (proxy_url, handle) = bridge.bind_proxy_with_tls_profile("chrome_131").await?;
// Pass proxy_url to your browser context; handle tracks the session outcome.
handle.mark_success();
```

If no proxy in the pool satisfies the requested profile,
`ProxyError::AllProxiesUnhealthy` is returned — the same error returned
when no healthy proxy of any kind is available.

---

## Failure tracking across integrations

`ProxyHandle` uses RAII to track request outcomes. The same contract applies
regardless of whether it came from a `graph` or `browser` integration:

1. Acquire a handle from `acquire_proxy()` (graph) or `bind_proxy()` (browser).
2. Perform the I/O operation.
3. Call `handle.mark_success()` on success.
4. Drop the handle (success or failure is recorded in the circuit breaker).

If the handle is dropped without `mark_success()`, the failure counter
increments. After `circuit_open_threshold` consecutive failures the circuit
opens and the proxy is skipped until after the `circuit_half_open_after` cooldown.

When the manager is built with
[`ProxyManager::with_thompson_sampling`](https://docs.rs/stygian-proxy/0.14/stygian_proxy/struct.ProxyManager.html#method.with_thompson_sampling),
the same `mark_success` / drop-failure signals also feed the Bayesian
bandit (see [Thompson-sampling Bayesian rotation](strategies.md#thompsonsampling-bayesian-rotation)).

---

## Per-vendor sticky browser integration (`vendor-stickiness` feature)

When the `vendor-stickiness` cargo feature is enabled, a browser bridge
can opt into per-vendor sticky bindings by calling
`acquire_for_domain_with_vendor`:

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{
    MemoryProxyStore, ProxyConfig, ProxyManager,
    stickiness::VendorStickinessMap,
};
use stygian_proxy::types::VendorId;

let storage = Arc::new(MemoryProxyStore::default());
let manager = ProxyManager::with_round_robin(storage, ProxyConfig::default())?
    .builder()
    .stickiness_map(VendorStickinessMap::with_builtin_defaults())
    .build()?;

// Akamai's 30-minute sticky policy applies for "store.example.com".
let handle = manager
    .acquire_for_domain_with_vendor("store.example.com", VendorId::Akamai)
    .await?;
```

Pair this with `BrowserProxySource` in production:

```rust,no_run
use stygian_proxy::browser::ProxyManagerBridge;
use stygian_browser::{BrowserConfig, WaitUntil};
use std::time::Duration;

let bridge = ProxyManagerBridge::new(Arc::clone(&manager));
let config = BrowserConfig::builder()
    .proxy_source(bridge) // <- ProxySource trait, not the free function
    .headless(true)
    .build();

// Akamai-guarded sites get the 30-minute sticky binding;
// DataDome-guarded sites get fresh-per-request.
let page = config.acquire().await?;
page.navigate(
    "https://store.example.com/login",
    WaitUntil::DomContentLoaded,
    Duration::from_secs(30),
).await?;
```

See the [Sticky Sessions](sticky-sessions.md) chapter for the full
policy matrix and `VendorStickinessMap` builder API.

---

## Network-identity coherence check (`coherence-validation` feature)

`CoherenceValidator` is a `Box<dyn CoherencePort>` that gates acquisition
on the WebRTC + DNS + timezone + locale + Accept-Language five-vector
match. It is composed onto a manager via the builder:

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{
    CoherenceContext, CoherencePolicy, CoherenceValidator,
    MemoryProxyStore, MismatchField, ProxyConfig, ProxyManager,
};

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?
    .builder()
    .coherence_validator(Arc::new(CoherenceValidator::default()))
    .build()?;
```

At acquire time:

```rust,no_run
let ctx = CoherenceContext::from_browser_page(&page).await?;
let policy = CoherencePolicy::hard_fail_on(MismatchField::WebRtcPublicIp);
let handle = mgr.acquire_proxy_with_coherence(&ctx, &policy).await?;
```

`policy.advisory()` (the default) logs mismatches at `tracing::warn!` and
still issues the proxy. `policy.hard_fail_on(field)` upgrades the
chosen field to a hard reject — the call returns
`ProxyError::CoherenceMismatch { field, observed, expected }` and no
proxy is leased. Zero allocation per call once the `CoherenceContext`
is built; safe on the hot path.

---

## Thompson-sampling rotation wiring

When the `bayesian-rotation` feature is enabled, the manager is built
with `ThompsonStrategy` and observers are fed by the existing
`ProxyHandle::mark_success` / drop-failure path — no separate observer
call is required at the call site. To seed the bandit from a known-good
feed, use `strategy_warmup_observe`:

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

// Seed the bandit from a known-good feed so cold-start traffic
// is already informed.
mgr.strategy_warmup_observe(proxy_id_a, true).await;
mgr.strategy_warmup_observe(proxy_id_b, false).await;
```

`strategy_warmup_observe` is fire-and-forget — the bandit reads from
these observations on its next acquire. In production you typically
seed at startup from a curated trust list and let live traffic update
the posterior.

---

## Ingest with metadata

`add_proxy_with_metadata` is a convenience constructor that validates
the URL against `vendor_quirks::check` and accepts `(url, asn, city,
postal_code)` directly without a `Proxy` struct literal:

```rust,no_run
use stygian_proxy::types::well_known;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use std::sync::Arc;

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;

mgr.add_proxy_with_metadata(
    "http://user:pass@edge1.example.com:8080".into(),
    well_known::KNOWN_ASN_AKAMAI, // 20_940
    "Cambridge".into(),
    "02142".into(),
).await?;
```

The metadata is stored on the underlying `Proxy`'s `capabilities.{asn,
city, postal_code}` fields and participates in
`CapabilityRequirement::require_asn` / `require_city` /
`require_postal_code` filters at acquire time.

---

## Vendor-quirk ingest validation

Provider-specific URL traps (Crawlera / Zyte port 8011 plain-HTTP
errors, Bright Data / IPRoyal username-format warnings) surface late —
deep in the TLS handshake or on the first request — without warning.
`vendor_quirks::check` validates at ingest:

```rust,no_run
use stygian_proxy::vendor_quirks::{check, ProxyUrl, QuirkSeverity};

let url = ProxyUrl::parse("http://proxy.crawlera.com:8011").unwrap();
for m in check(&url) {
    match m.severity {
        QuirkSeverity::Error  => {
            return Err(format!("rejected: {}", m.description).into());
        }
        QuirkSeverity::Warning => tracing::warn!(?m, "vendor-quirk"),
        QuirkSeverity::Info    => tracing::info!(?m, "vendor-quirk"),
    }
}
```

`ProxyManager::add_proxy_with_metadata` and the free-list fetchers
internally call this validation; if you build your own ingest
pipeline, call it before `add_proxy`. Error-severity quirks should
reject the proxy outright; warning-severity quirks should be logged
and accepted.
