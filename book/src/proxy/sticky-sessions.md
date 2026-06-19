# Sticky Sessions

Without session stickiness, every proxy-rotation call may select a different exit IP.
Most sites treat the IP as part of the session identity. A login flow that acquires a
cookie from IP A and then submits the form from IP B will fail — or worse, trigger a
suspicious-behaviour alarm.

*Sticky sessions* bind a target domain to one proxy for a configurable duration, so
all requests to the same site use the same exit IP for the lifetime of the binding.
When the binding expires the next request automatically picks a fresh proxy and pins
the new one.

The 0.14.0 release adds **per-vendor** stickiness on top of the existing
per-domain API — different anti-bot vendors want different strategies, and
`PerimeterX` / `Kasada` are typically better off with a *fresh per domain*
strategy while `Akamai` is better off with a *sticky for TTL* strategy.

---

## Quick reference

There are now two orthogonal stickiness surfaces in the crate:

- **`ProxyConfig::sticky_policy`** — the legacy domain-only path, always
  compiled, controls `acquire_for_domain()`. Use this for simple "all
  requests to this domain share an IP for 5 minutes" cases.
- **`ProxyManagerBuilder::stickiness_map(VendorStickinessMap)`** — the
  new per-vendor path, gated behind the `vendor-stickiness` cargo
  feature, controls `acquire_for_domain_with_vendor()`. Use this when
  you know which anti-bot vendor protects a target domain.

The two compose: the per-vendor path consults the stickiness map, and the
per-domain path still applies its TTL on top.

---

## Per-domain stickiness (always compiled)

Sticky-session behaviour is controlled by `ProxyConfig::sticky_policy`:

```rust,no_run
use std::time::Duration;
use stygian_proxy::{ProxyConfig, session::StickyPolicy};

let config = ProxyConfig {
    sticky_policy: StickyPolicy::domain(Duration::from_secs(600)), // 10-minute sessions
    ..ProxyConfig::default()
};
```

| Policy variant | Description |
| --- | --- |
| `StickyPolicy::Disabled` (default) | No binding — every call may use a different proxy |
| `StickyPolicy::Domain { ttl }` | Bind per domain name; TTL controls how long the binding lives |

The default TTL for `StickyPolicy::domain_default()` is **5 minutes**.

### Using per-domain sticky sessions

Once the policy is set, use `acquire_for_domain` instead of `acquire_proxy`:

```rust,no_run
use std::{sync::Arc, time::Duration};
use stygian_proxy::{
    ProxyConfig, ProxyManager, ProxyType, Proxy,
    session::StickyPolicy,
    storage::MemoryProxyStore,
};

let storage = Arc::new(MemoryProxyStore::default());
let config  = ProxyConfig {
    sticky_policy: StickyPolicy::domain(Duration::from_secs(600)),
    ..ProxyConfig::default()
};
let mgr = ProxyManager::with_round_robin(storage, config)?;

// Add proxies to the pool …
mgr.add_proxy(Proxy {
    url:        "http://residential-proxy:8080".into(),
    proxy_type: ProxyType::Http,
    username:   Some("user".into()),
    password:   Some("pass".into()),
    weight:     1,
    tags:       vec!["residential".into()],
    ..Default::default()
}).await?;

// ── Login flow ────────────────────────────────────────────────────────────────

// All three calls reuse the same proxy for "store.example.com"
// as long as the 10-minute TTL has not elapsed.

let handle = mgr.acquire_for_domain("store.example.com").await?;
let _ = client_with_proxy(&handle).get("https://store.example.com/login").send().await?;
handle.mark_success();

let handle = mgr.acquire_for_domain("store.example.com").await?;
let _ = client_with_proxy(&handle)
    .post("https://store.example.com/session")
    .form(&[("user", "alice"), ("pass", "hunter2")])
    .send()
    .await?;
handle.mark_success();

let handle = mgr.acquire_for_domain("store.example.com").await?;
let _ = client_with_proxy(&handle).get("https://store.example.com/account").send().await?;
handle.mark_success();
# Ok(())
```

---

## Per-vendor stickiness (feature `vendor-stickiness`)

Different vendors reward different strategies. The 2026 scraping guide
encodes the matrix as built-in defaults; override individual entries
when you have a strong opinion.

### Built-in defaults

| Vendor | Default policy | Rationale |
| --- | --- | --- |
| `Akamai` | `StickyForTtl { ttl: 30 min }` | Akamai Bot Manager degrades gracefully on sticky sessions and aggressively scores IP churn |
| `Cloudflare` | `StickyForTtl { ttl: 5 min }` | Cloudflare's bot-score model includes session continuity; full-session stickiness is overkill |
| `Imperva` | `StickyForTtl { ttl: 15 min }` | Imperva sits between Akamai and Cloudflare on stickiness benefit |
| `PerimeterX` | `FreshPerDomain` | PerimeterX tracks session-bound device fingerprints; a fresh IP per domain disrupts the signal |
| `Kasada` | `FreshPerDomain` | Same rationale as PerimeterX |
| `DataDome` | `FreshPerRequest` | DataDome aggressively fingerprints IP churn; never reuse |
| Anything else (including `VendorId::Unknown`) | `FreshPerRequest` | Safest default — fail open to fresh |

### Customising the map

```rust,no_run
use std::time::Duration;
use stygian_proxy::stickiness::{StickinessPolicy, VendorStickinessMap};
use stygian_proxy::types::VendorId;

let map = VendorStickinessMap::with_builtin_defaults()
    // Force Akamai to never rotate during the process lifetime.
    .with_override(VendorId::Akamai, StickinessPolicy::StickyForever)
    // Per-domain fresh rotation for a vendor not in the built-in matrix.
    .with_override(VendorId::Hcaptcha, StickinessPolicy::FreshPerDomain);
```

`VendorStickinessMap` is a transparent newtype around
`BTreeMap<VendorId, StickinessPolicy>`. Lookups via
`map.for_vendor(vendor)` fall back to `FreshPerRequest` for any vendor
that has no entry — that is the **safest default** and matches the
"unknown vendor" row of the built-in table.

### Installing on a `ProxyManager`

```rust,no_run
use std::sync::Arc;
use stygian_proxy::{
    MemoryProxyStore, ProxyConfig, ProxyManager,
    stickiness::VendorStickinessMap,
};
use stygian_proxy::types::VendorId;

let storage = Arc::new(MemoryProxyStore::default());
let mgr = ProxyManager::with_round_robin(storage, ProxyConfig::default())?
    .builder()
    .stickiness_map(VendorStickinessMap::with_builtin_defaults())
    .build()?;
```

The builder method is feature-gated. When the `vendor-stickiness`
feature is off, `ProxyManagerBuilder::stickiness_map` does not exist;
calling `acquire_for_domain_with_vendor` returns
`ProxyError::VendorStickinessDisabled`.

### Acquiring with a vendor tag

```rust,no_run
use stygian_proxy::types::VendorId;

// Akamai's 30-minute sticky policy applies for this call.
let handle = mgr.acquire_for_domain_with_vendor(
    "store.example.com",
    VendorId::Akamai,
).await?;
```

`SessionDecision` describes the outcome:

| Variant | Meaning |
| --- | --- |
| `UseSticky(Uuid)` | The vendor's policy returned an existing binding; the inner `Uuid` is the bound proxy |
| `AcquireFresh` | No active binding; the manager picked a fresh proxy from the pool |
| `AcquireAndBind(Duration)` | No active binding; the manager picked a fresh proxy and bound it for the inner TTL |

### Unknown-vendor fallback

`VendorId::Unknown` is a real value (used when the operator has not
classified a domain). The built-in map returns
`StickinessPolicy::FreshPerRequest` for it, which is the safe default.

---

## Session lifecycle

```mermaid
stateDiagram-v2
    [*] --> Unbound : First request to domain

    Unbound --> Bound : acquire_for_domain()\nor acquire_for_domain_with_vendor()\nselects proxy via strategy\nbind(domain, proxy_id, ttl)

    Bound --> Bound : acquire_for_domain()\nlookup() hit — same proxy returned

    Bound --> Expired : TTL elapsed\n(or FreshPerDomain / FreshPerRequest policy)

    Expired --> Bound : acquire_for_domain()\npicks fresh proxy\nbinds new session

    Bound --> Unbound : ProxyHandle dropped\nwithout mark_success()\nunbind(domain) called

    Bound --> Unbound : proxy removed from pool\nor circuit breaker open\nsession invalidated

    note right of Bound
        Per-vendor policy
        consults VendorStickinessMap
        before reusing the binding
    end note
```

For `FreshPerDomain` the "expired" transition fires on every call to the
same domain — there is no reuse within a domain, but a different domain
keeps its own binding.

For `FreshPerRequest` there is no `Bound` state at all — every call goes
through `AcquireFresh`.

---

## Failure handling

`ProxyHandle` is a RAII guard. When it is dropped without calling `mark_success()`,
the sticky session for that domain is **automatically invalidated** and the circuit
breaker records a failure:

```rust,no_run
let handle = mgr.acquire_for_domain("shop.example.com").await?;

// If the request fails or the guard is dropped without mark_success(),
// the domain session is cleared and the circuit breaker is incremented.
// The next call to acquire_for_domain picks a fresh proxy.
let resp = client.get("https://shop.example.com/checkout").send().await?;

if resp.status().is_success() {
    handle.mark_success();  // binding stays alive for the remainder of the TTL
}
// else: drop without mark_success → session reset automatically
```

---

## Low-level SessionMap API

`SessionMap` can also be used standalone, outside of `ProxyManager`, when you need
fine-grained control:

```rust,no_run
use std::time::Duration;
use uuid::Uuid;
use stygian_proxy::session::SessionMap;

let sessions = SessionMap::new();
let proxy_id = Uuid::new_v4();

// Bind "login.example.com" to a proxy for 5 minutes.
sessions.bind("login.example.com", proxy_id, Duration::from_secs(300));

// Lookup returns Some(id) while the session is active.
assert_eq!(sessions.lookup("login.example.com"), Some(proxy_id));

// Purge all expired entries — safe to call on any schedule.
let removed = sessions.purge_expired();
println!("purged {removed} expired sessions");

// Manually invalidate a binding.
sessions.unbind("login.example.com");
```

| Method | Description |
| --- | --- |
| `bind(domain, proxy_id, ttl)` | Create or overwrite a domain binding |
| `lookup(domain) -> Option<Uuid>` | Return the bound proxy ID, or `None` if expired |
| `unbind(domain)` | Remove a binding immediately |
| `purge_expired() -> usize` | Evict all expired bindings; returns count removed |
| `active_count() -> usize` | Number of non-expired bindings currently held |
| `acquire_session(domain, vendor, policy_map)` *(feature `vendor-stickiness`)* | Combine the legacy `lookup` with the per-vendor `VendorStickinessMap` decision logic |

---

## Pool stats

`ProxyManager::pool_stats()` includes sticky session state:

```rust,no_run
let stats = mgr.pool_stats().await?;
println!("total proxies:    {}", stats.total);
println!("healthy proxies:  {}", stats.healthy);
println!("circuit open:     {}", stats.open);
println!("active sessions:  {}", stats.active_sessions);
```

---

## Multi-domain scraping

When you scrape many domains concurrently, each gets its own independent binding.
The `ProxyManager` session map is an `Arc<RwLock<HashMap<String, ...>>>` so concurrent
lookups never block each other:

```rust,no_run
// Different domains → different proxies, each bound separately.
let h1 = mgr.acquire_for_domain("shop-a.com").await?;
let h2 = mgr.acquire_for_domain("shop-b.com").await?;
let h3 = mgr.acquire_for_domain("shop-c.com").await?;

// All three run in parallel — each gets its own sticky proxy.
tokio::join!(
    fetch(&h1, "https://shop-a.com/products"),
    fetch(&h2, "https://shop-b.com/products"),
    fetch(&h3, "https://shop-c.com/products"),
);
```

When the per-vendor path is enabled, `acquire_for_domain_with_vendor(domain, vendor)`
treats `(domain, vendor)` as the binding key — so `acquire_for_domain_with_vendor("shop-a.com", VendorId::Akamai)`
and `acquire_for_domain_with_vendor("shop-a.com", VendorId::Cloudflare)` produce
independent bindings under the same domain name.
