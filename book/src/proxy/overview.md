# Proxy Rotation Overview

`stygian-proxy` is a high-performance, resilient proxy rotation library for the Stygian
scraping ecosystem. It manages a pool of proxy endpoints, tracks per-proxy health and
latency metrics, learns per-domain effectiveness online, and integrates directly with
both `stygian-graph` HTTP adapters and `stygian-browser` page contexts.

---

## Feature summary

| Feature | Description |
| --- | --- |
| **Rotation strategies** | Round-robin, random, weighted (by proxy weight), least-used (by request count), and Thompson-sampling Bayesian (feature `bayesian-rotation`) |
| **Per-proxy metrics** | Atomic latency and success-rate tracking — zero lock contention |
| **Async health checker** | Configurable-interval background task; each proxy probed concurrently via `JoinSet` |
| **Health-check jitter** | Per-cycle random ±N% interval spread via `health_check_jitter_pct` — prevents thundering-herd against shared targets |
| **Circuit breaker** | Per-proxy lock-free FSM: `Closed → Open → HalfOpen`; auto-recovery after cooldown |
| **Capability filtering** | Filter at acquire time by TLS profile, CDN-edge, SOCKS UDP relay, HTTP/3 tunnel, geo country, **IP class**, **target vendor**, **ASN**, **city**, **postal code** |
| **IP-class taxonomy** | `Mobile` / `Isp` / `Residential` / `Datacenter` / `Unknown` — operator-declared egress tier; rank-based "at least this tier" requirements |
| **Vendor compatibility** | `TargetVendorCompatibility` (Preferred / Acceptable / Marginal / Blocked) on `Proxy` and `ProxyCapabilities`; capability-aware acquisition |
| **Vendor stickiness** | Per-vendor `StickinessPolicy` (feature `vendor-stickiness`) — built-in defaults encode the 2026 anti-bot matrix |
| **Network-identity coherence** | `CoherencePort` + `DefaultCoherenceValidator` (feature `coherence-validation`) — catches the WebRTC + DNS + timezone + locale + Accept-Language five-vector mismatch |
| **Geo metadata** | `add_proxy_with_metadata(url, asn, city, postal_code)` and `ProxyCapabilities::{asn, city, postal_code}` for Infatica-style fine-grained geo routing |
| **Vendor-quirk ingest validation** | `vendor_quirks::check` rejects provider-specific URL traps at ingest time (Crawlera/Zyte port 8011, Bright Data / IPRoyal username formats) |
| **CDN-edge proxy type** | `ProxyType::CdnEdge` for CDN-fronted egress nodes alongside `Http`, `Https`, `Socks4`, `Socks5` |
| **Persistent connections** | `TransportPreference::PersistentTcp` with configurable max-requests and connection max-age |
| **TLS profile binding** | `ProxyManagerBridge::bind_proxy_with_tls_profile` ties a browser context to a proxy whose `tls_profile` matches the browser fingerprint |
| **In-memory pool** | No external database required; satisfies the `ProxyStoragePort` trait |
| **graph integration** | `ProxyManagerPort` trait for `stygian-graph` HTTP adapters (feature `graph`) |
| **browser integration** | Per-context proxy binding for `stygian-browser` (feature `browser`) |
| **SOCKS support** | `Socks4` and `Socks5` proxy types (feature `socks`) |
| **DNS TXT discovery** | `DnsTxtFetcher` resolves proxy lists from DNS TXT records (feature `dns-fetcher`) |

---

## Quick start

Add the dependency:

```toml
[dependencies]
stygian-proxy = { version = "0.14", features = ["graph"] }
```

Build a pool and make a request:

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::types::{Proxy, ProxyType, IpClass, TargetVendorCompatibility};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage = Arc::new(MemoryProxyStore::default());
    let manager = Arc::new(
        ProxyManager::with_round_robin(storage, ProxyConfig::default())?
    );

    // Register a mobile-carrier residential proxy with vendor metadata.
    manager.add_proxy(Proxy {
        url: "http://proxy1.example.com:8080".into(),
        proxy_type: ProxyType::Http,
        username: None,
        password: None,
        weight: 1,
        tags: vec!["us-east".into()],
        ip_class: IpClass::Residential,
        target_compatibility: TargetVendorCompatibility::preferred(),
        ..Default::default()
    }).await?;

    // Start background health checks.
    let (cancel, _task) = manager.start();

    // Acquire a proxy for a request.
    let handle = manager.acquire_proxy().await?;
    println!("using proxy: {}", handle.proxy_url);

    // Signal success — omitting this counts as a failure toward the circuit breaker.
    handle.mark_success();

    cancel.cancel(); // stop health checker
    Ok(())
}
```

---

## Architecture

```
┌──────────────────────────────────────────────┐
│                ProxyManager                  │
│                                              │
│  ┌───────────────┐  ┌─────────────────────┐  │
│  │ HealthChecker │  │   CircuitBreakers   │  │
│  │   + jitter    │  │     (per proxy)     │  │
│  └───────────────┘  └─────────────────────┘  │
│                                              │
│  ┌────────────────────────────────────────┐  │
│  │           RotationStrategy             │  │
│  │  RoundRobin / Random / Weighted /      │  │
│  │  LeastUsed / ThompsonSampling          │  │
│  └────────────────────────────────────────┘  │
│                                              │
│  ┌────────────────────────────────────────┐  │
│  │      CapabilityRequirement filter      │  │
│  │  tls_profile / cdn_edge / ip_class /   │  │
│  │  vendor / asn / city / postal_code     │  │
│  └────────────────────────────────────────┘  │
│                                              │
│  ┌────────────────────────────────────────┐  │
│  │    CoherenceValidator (optional)       │  │
│  │    WebRTC + DNS + tz + locale + lang   │  │
│  └────────────────────────────────────────┘  │
│                                              │
│  ┌────────────────────────────────────────┐  │
│  │  VendorStickinessMap (optional)        │  │
│  │  per-vendor sticky / fresh-per-domain  │  │
│  └────────────────────────────────────────┘  │
│                                              │
│  ┌────────────────────────────────────────┐  │
│  │          ProxyStoragePort              │  │
│  │     MemoryProxyStore (built-in)        │  │
│  └────────────────────────────────────────┘  │
└──────────────────────────────────────────────┘
         │                       │
         ▼                       ▼
   stygian-graph           stygian-browser
   HTTP adapters           page contexts
```

`ProxyManager` is the main entry point. It composes a storage backend, a rotation
strategy, a background health checker, a map of per-proxy circuit breakers, and — when
the corresponding cargo features are enabled — an optional coherence validator and a
per-vendor stickiness map. Callers interact primarily via
`acquire_proxy()` → `ProxyHandle` → `mark_success()`.

---

## Cargo features

| Feature | Enables |
| --- | --- |
| *(default: none)* | Core pool, strategies, health checker, circuit breaker |
| `graph` | `ProxyManagerPort` trait + blanket impl + `NoopProxyManager` |
| `browser` | `BrowserProxySource` trait + `ProxyManagerBridge` |
| `socks` | `ProxyType::Socks4` and `ProxyType::Socks5` variants |
| `tls-profiled` | `tls_profile` field on `ProxyCapabilities` + `bind_proxy_with_tls_profile` |
| `mcp` | MCP-server tool surface for proxy pool inspection |
| `dns-fetcher` | `DnsTxtFetcher` (resolves proxy lists from DNS TXT records via `hickory-resolver`) |
| `bayesian-rotation` | `ThompsonStrategy` rotation + Bayesian observer wiring into `ProxyHandle::mark_success` |
| `coherence-validation` | `CoherencePort` trait + `DefaultCoherenceValidator` (WebRTC + DNS + tz + locale + lang five-vector check) |
| `vendor-stickiness` | `StickinessPolicy` / `VendorStickinessMap` per-vendor sticky session routing |
| `full` | Aggregator that turns on every feature above |

---

## ProxyConfig defaults

| Field | Default | Description |
| --- | --- | --- |
| `health_check_url` | `https://httpbin.org/ip` | URL probed to verify liveness |
| `health_check_interval` | 60 s | How often to run checks |
| `health_check_timeout` | 5 s | Per-probe HTTP timeout |
| `circuit_open_threshold` | 5 | Consecutive failures before circuit opens |
| `circuit_half_open_after` | 30 s | Cooldown before attempting recovery |
| `health_check_jitter_pct` | `0.10` | ±10% random spread on probe intervals — anti-thundering-herd |
| `tls_profiled_request_mode` | `Disabled` *(with `tls-profiled`)* | Per-request TLS profile application mode |

---

## Capability-aware acquisition

Beyond the legacy `tags`-based filter, `CapabilityRequirement` is the primary way
to declare what a request needs from a proxy. The struct carries one `Option`
field per constraint; all fields are independently `#[serde(default,
skip_serializing_if = "Option::is_none")]` so a serialised requirement with
no filters round-trips cleanly.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};
use stygian_proxy::types::{
    CapabilityRequirement, IpClassRequirement, ProxyType, VendorId,
};

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;

let req = CapabilityRequirement {
    target_vendor: Some(VendorId::Akamai),
    require_ip_class: Some(IpClassRequirement::at_least(stygian_proxy::types::IpClass::Residential)),
    require_https_connect: Some(true),
    require_cdn_edge: None,
    require_tls_profile: Some("chrome_131".into()),
    require_asn: Some(20_940),              // Akamai
    require_city: Some("Cambridge".into()),
    require_postal_code: None,
    require_geo_country: None,
    require_socks5_udp: None,
    require_http3_tunnel: None,
};
let handle = mgr.acquire_with_capabilities(&req).await?;
```

If no candidate in the pool satisfies the requirement, `acquire_with_capabilities`
returns `ProxyError::NoCompatibleProxy` (distinct from
`ProxyError::AllProxiesUnhealthy`, which signals every proxy is currently
circuit-open rather than structurally incompatible).

---

## Geo metadata & ingest validation

`ProxyCapabilities` carries three optional geo fields that acquisition can
filter on: `asn: Option<u32>`, `city: Option<String>`,
`postal_code: Option<String>`. `Proxy` carries the same fields so operators
can curate metadata at ingest time without going through `ProxyCapabilities`.

```rust,no_run
use stygian_proxy::types::well_known;
use std::sync::Arc;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;

mgr.add_proxy_with_metadata(
    "http://user:pass@proxy.example.com:8080".into(),
    well_known::KNOWN_ASN_AKAMAI,        // 20_940
    "Cambridge".into(),
    "02142".into(),
).await?;
```

Ingest validates the metadata at the call site:

- `validate_asn(u32) -> Result<(), ProxyError>` — rejects `0` and `u32::MAX`.
- `validate_city(&str) -> Result<(), ProxyError>` — rejects empty strings.
- `validate_postal_code(&str) -> Result<(), ProxyError>` — rejects empty strings.

The `well_known` submodule exposes canonical ASNs for the most common
anti-bot vendor networks and CDN providers as `pub const u32` values, with an
`ALL_KNOWN_ASNS: &[u32]` companion slice for "is this ASN one we recognise"
checks:

| Const | Value | Vendor |
| --- | --- | --- |
| `KNOWN_ASN_CLOUDFLARE` | 13_335 | Cloudflare |
| `KNOWN_ASN_AKAMAI` | 20_940 | Akamai |
| `KNOWN_ASN_FASTLY` | 54_113 | Fastly |
| `KNOWN_ASN_CLOUDFRONT` | 16_509 | AWS CloudFront |
| `KNOWN_ASN_GOOGLE` | 15_169 | Google |
| `KNOWN_ASN_AZURE` | 8_075 | Microsoft Azure |
| `KNOWN_ASN_LIMELIGHT` | 22_822 | Limelight |
| `KNOWN_ASN_HIGHWINDS` | 20_446 | Highwinds |
| `KNOWN_ASN_EDGECAST` | 15_133 | Edgecast / Verizon Digital Media |
| `KNOWN_ASN_SUCURI` | 51_167 | Sucuri |
| `KNOWN_ASN_OVH` | 16_276 | OVH |
| `KNOWN_ASN_HETZNER` | 24_940 | Hetzner |
| `KNOWN_ASN_DIGITALOCEAN` | 14_061 | DigitalOcean |
| `KNOWN_ASN_LINODE` | 63_949 | Linode (Akamai Connected Cloud) |
| `KNOWN_ASN_VULTR` | 204_957 | Vultr |

---

## Vendor-specific URL quirks

Provider-specific URL formats silently break scrapers in characteristic ways.
The Crawlera and Zyte `:8011` plain-HTTP endpoints, for example, sit behind a
TLS terminator and crash with `BoringSSL WRONG_VERSION_NUMBER` if the proxy
adapter forwards them without TLS — failing late, deep in the TLS handshake.

`vendor_quirks::check` runs at ingest time and returns a `Vec<QuirkMatch>`
describing any quirks that apply to a parsed `ProxyUrl`. Error-severity quirks
are rejected outright (the proxy never enters the pool); warning-severity
quirks are accepted and logged.

```rust,no_run
use stygian_proxy::vendor_quirks::{check, ProxyUrl, Scheme, QuirkSeverity};

let url = ProxyUrl::parse("http://proxy.crawlera.com:8011").unwrap();
for m in check(&url) {
    match m.severity {
        QuirkSeverity::Error  => eprintln!("rejected: {}", m.description),
        QuirkSeverity::Warning => tracing::warn!(?m, "vendor-quirk"),
        QuirkSeverity::Info    => tracing::info!(?m, "vendor-quirk"),
    }
}
```

The built-in `VENDOR_QUIRKS` table seeds four entries:

| Quirk | Host suffix | Port | Required scheme | Severity |
| --- | --- | --- | --- | --- |
| `CRAWLERA_8011_QUIRK` | `crawlera.com` | 8011 | `Https` | **Error** |
| `ZYTE_8011_QUIRK` | `zyte.com` | 8011 | `Https` | **Error** |
| `BRD_SUPERPROXY_QUIRK` | `brd.superproxy.io` | 22225 | `Http` | Warning |
| `IPROYAL_QUIRK` | `iproyal.com` | 12321 | `Http` | Warning |

Quirk descriptions are static `&'static str` literals that do not echo any
credential component.

---

## Network-identity coherence

A clean fingerprint that disagrees with the egress network identity (WebRTC
leaking the real IP, Accept-Language not matching the proxy country, DNS
resolver living in a different jurisdiction) is the most commonly overlooked
leak cited by the 2026 scraping guide. `CoherencePort` + `DefaultCoherenceValidator`
checks the five-vector match at the orchestration layer before any request is
sent.

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{
    CoherenceContext, CoherencePolicy, CoherenceValidator, MismatchField,
    MemoryProxyStore, ProxyConfig, ProxyManager,
};

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?
    .with_coherence_validator(Arc::new(CoherenceValidator::default()))?;

let ctx = CoherenceContext::from_browser_page(&page).await?;
let policy = CoherencePolicy::hard_fail_on(MismatchField::WebRtcPublicIp);
let handle = mgr.acquire_proxy_with_coherence(&ctx, &policy).await?;
```

`CoherencePolicy` is an advisory-or-hard-fail severity model: by default the
validator logs mismatches at `tracing::warn!` and still issues the proxy;
`hard_fail_on(field)` upgrades the chosen field to a hard reject. The
`MismatchField` enum lists the five vectors: `ProxyGeoVsDns`,
`WebRtcPublicIp`, `Timezone`, `Locale`, `AcceptLanguage`.

The validator runs in-process with **zero allocation per call** once the
`CoherenceContext` is built — it is safe to run on the hot acquisition path.

---

## Thompson-sampling Bayesian rotation

Round-robin and weighted strategies pick blindly. On protected-target
workloads, that wastes requests on proxies the target has already flagged.
`ThompsonStrategy` learns per-proxy health online and concentrates traffic on
proxies whose posterior `Beta(α, β)` distribution favours success.

Cites **76% success** vs **36% round-robin** on identical proxies in the
internal `ProxyOps` benchmark (549,114 requests over 7 days). Per-proxy
counters use `AtomicU64`; the hot-path acquire stays sub-microsecond.

```rust,no_run
use std::sync::Arc;
use std::time::Duration;
use stygian_proxy::{MemoryProxyStore, ProxyConfig, ProxyManager};

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_thompson_sampling(
    storage,
    ProxyConfig::default(),
    Duration::from_secs(300), // decay_interval — 5 min default
)?;

// Optional: seed the bandit from a known-good feed so cold-start
// traffic is already informed.
mgr.strategy_warmup_observe(proxy_id_a, true).await;
mgr.strategy_warmup_observe(proxy_id_b, false).await;
```

When the strategy is enabled, `ProxyHandle::mark_success` and the implicit
"failure on drop" path both feed the bandit — there is no separate observer
step required at the call site. The `decay_interval` (default 5 min) and
`decay_factor` (default 0.95) knobs control how quickly stale observations
age out of the posterior. A prior-bias seam lets the strategy weight
proxies whose `TargetVendorCompatibility` is higher for the target vendor.

See [`ThompsonStrategy`](https://docs.rs/stygian-proxy/0.14/stygian_proxy/strategy/thompson/struct.ThompsonStrategy.html)
for the full type.
