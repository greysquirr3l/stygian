# Ecosystem Integrations

`stygian-proxy` can be wired into both `stygian-graph` HTTP adapters and
`stygian-browser` page contexts via optional Cargo features.

---

## stygian-graph (`graph` feature)

Enable the `graph` feature to get `ProxyManagerPort` — the trait that decouples
graph HTTP adapters from any specific proxy implementation.

```toml
[dependencies]
stygian-proxy = { version = "0.1", features = ["graph"] }
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
stygian-proxy = { version = "0.1", features = ["browser"] }
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
