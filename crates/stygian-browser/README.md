# stygian-browser

High-performance, anti-detection browser automation library for Rust.

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](../../LICENSE)
[![Coverage](https://img.shields.io/badge/coverage-limited%20by%20CDP-lightgrey)](https://github.com/greysquirr3l/stygian/actions)

Built on the [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/) via
[`chromiumoxide`](https://github.com/mattsse/chromiumoxide) with comprehensive stealth features
for bypassing modern anti-bot systems: Cloudflare, `DataDome`, `PerimeterX`, Akamai.

---

## Features

| Feature | Description | Default |
| --------- | ------------- | --------- |
| `stealth` | Navigation spoofing, canvas noise, WebGL randomization, CDP protection | ✓ |
| `tls-config` | TLS fingerprint profiling via rustls (requires `stealth`) | — |
| `mcp` | MCP (Model Context Protocol) tools | — |
| `metrics` | Prometheus metrics exporter | — |
| `extract` | Structured data extraction via `#[derive(Extract)]` | — |
| `similarity` | Similarity scoring for duplicate detection | — |
| `full` | All features enabled | — |

---

## Features

---

## Installation

```toml
[dependencies]
stygian-browser = "*"
tokio = { version = "1", features = ["full"] }
```

Enable (or disable) stealth features:

```toml
[dependencies]
# stealth is the default feature; disable for a minimal build
stygian-browser = { version = "*", default-features = false }
```

---

## Quick Start

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build a config — defaults are headless Chrome with Advanced stealth
    let config = BrowserConfig::default();

    // Launch a warm pool (2 browsers ready immediately)
    let pool = BrowserPool::new(config).await?;

    // Acquire a browser handle (< 100 ms from warm pool)
    let handle = pool.acquire().await?;

    // Open a tab and navigate
    let mut page = handle.browser().unwrap().new_page().await?;
    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    println!("Title: {}", page.title().await?);

    // Release the browser back to the pool
    handle.release().await;
    Ok(())
}
```

---

## Configuration

`BrowserConfig` controls every aspect of browser launch, anti-detection, and pooling.

```rust,no_run
use stygian_browser::{BrowserConfig, StealthLevel};
use stygian_browser::config::PoolConfig;
use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
use std::time::Duration;

let config = BrowserConfig::builder()
    // Browser basics
    .headless(true)
    .window_size(1920, 1080)
    // Use a specific Chrome binary
    // .chrome_path("/usr/bin/google-chrome".into())
    // Stealth level
    .stealth_level(StealthLevel::Advanced)
    // Proxy (supports http/https/socks5)
    // .proxy("http://user:pass@proxy.example.com:8080".to_string())
    // WebRTC policy
    .webrtc(WebRtcConfig {
        policy: WebRtcPolicy::DisableNonProxied,
        ..Default::default()
    })
    // Pool settings
    .pool(PoolConfig {
        min_size: 2,
        max_size: 10,
        acquire_timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .build();
```

### Environment Variable Overrides

All config values can be overridden at runtime without recompiling:

| Variable | Default | Description |
| ---------- | --------- | ------------- |
| `STYGIAN_CHROME_PATH` | auto-detect | Path to Chrome/Chromium binary |
| `STYGIAN_HEADLESS` | `true` | `false` for headed mode |
| `STYGIAN_STEALTH_LEVEL` | `advanced` | `none`, `basic`, `advanced` |
| `STYGIAN_POOL_MIN` | `2` | Minimum warm browser count |
| `STYGIAN_POOL_MAX` | `10` | Maximum concurrent browsers |
| `STYGIAN_POOL_ACQUIRE_TIMEOUT_SECS` | `30` | Seconds to wait for pool slot |
| `STYGIAN_CDP_FIX_MODE` | `addBinding` | `addBinding`, `isolatedworld`, `enabledisable` |
| `STYGIAN_PROXY` | — | Proxy URL |
| `STYGIAN_DISABLE_SANDBOX` | auto-detect | `true` to pass `--no-sandbox` (see note below) |

---

## Stealth Levels

| Level | `navigator` spoof | Canvas noise | WebGL random | CDP protection | Human behavior |
| ------- | ----------------- | ------------ | ------------ | -------------- | -------------- |
| `None` | — | — | — | — | — |
| `Basic` | ✓ | — | — | ✓ | — |
| `Advanced` | ✓ | ✓ | ✓ | ✓ | ✓ |

**Trade-offs:**

- `None` — maximum performance, no evasion.  Suitable for sites with no bot detection.
- `Basic` — hides `navigator.webdriver`, masks the headless UA, enables CDP protection.
  Fast; appropriate for most scraping workloads.
- `Advanced` — full fingerprint injection (canvas noise, WebGL, audio, fonts, hardware
  concurrency, device memory), human-like mouse/keyboard events.  Adds ~10–30 ms overhead
  per page but passes all major detection suites.

---

## Browser Pool

The pool maintains a configurable number of warm browser instances and enforces
backpressure when all slots are occupied.

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool};
use stygian_browser::config::PoolConfig;
use std::time::Duration;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let config = BrowserConfig::builder()
    .pool(PoolConfig {
        min_size: 2,
        max_size: 8,
        idle_timeout: Duration::from_secs(300),
        acquire_timeout: Duration::from_secs(10),
    })
    .build();

let pool = BrowserPool::new(config).await?;
let stats = pool.stats();
println!("pool: {}/{} browsers, {} active", stats.available, stats.max, stats.active);
# Ok(())
# }
```

Browsers returned via `BrowserHandle::release()` go back into the pool automatically.
Browsers that fail their health check are discarded and replaced with fresh instances.

---

## Anti-Detection Techniques

### `navigator` Spoofing

- Overwrites `navigator.webdriver` to `undefined`
- Patches `navigator.plugins` with a realistic `PluginArray`
- Sets `navigator.languages`, `navigator.language`, `navigator.vendor`
- Aligns `navigator.hardwareConcurrency` and `navigator.deviceMemory` with the
  chosen device profile

### Canvas Fingerprint Noise

Adds sub-pixel noise (<1 px) to `HTMLCanvasElement.toDataURL()` and
`CanvasRenderingContext2D.getImageData()` — indistinguishable visually but unique
per page load.

### WebGL Randomisation

Randomises `RENDERER` and `VENDOR` WebGL parameter responses to prevent GPU-based
fingerprinting while keeping values plausible (real GPU family names are used).

### CDP Leak Protection

The Chrome `DevTools` Protocol itself can expose automation.  Three modes are
available via `CdpFixMode`:

| Mode | Protection | Compatibility |
| ------ | ----------- | --------------- |
| `AddBinding` | Wraps calls to hide `Runtime.enable` side-effects | Best overall |
| `IsolatedWorld` | Runs injection in a separate execution context | Moderate |
| `EnableDisable` | Toggles enable/disable around each command | Broad |

### Human-Like Behavior (Advanced only)

`MouseSimulator` generates Bézier-curve mouse paths with:

- Distance-aware step counts (12 steps for <100 px, up to 120 for >1000 px)
- Perpendicular control-point offsets for natural arc shapes
- Sub-pixel micro-tremor jitter (±0.3 px)
- 10–50 ms inter-event delays

`TypingSimulator` models:

- Per-key WPM variation (70–130 WPM base)  
- Configurable typo-and-correct rate
- Burst/pause rhythm typical of humans

---

## Integration with `stygian-proxy`

To use proxies from a `stygian-proxy` pool dynamically (at browser launch time):

```rust,no_run
use stygian_browser::BrowserConfig;
use stygian_proxy::{ProxyManager, MemoryProxyStore, browser::ProxyManagerBridge};
use stygian_proxy::types::ProxyConfig;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create proxy pool
    let manager = Arc::new(
        ProxyManager::with_round_robin(
            Arc::new(MemoryProxyStore::default()),
            ProxyConfig::default()
        )?
    );

    // Create bridge that implements ProxySource
    let bridge = Arc::new(ProxyManagerBridge::new(manager));

    // Pass to browser config
    let config = BrowserConfig::builder()
        .proxy_source(bridge)
        .build();

    // Each browser context will acquire its own proxy via the bridge
    let pool = BrowserPool::new(config).await?;
    let handle = pool.acquire().await?;

    // This browser is now routed through a proxy from the pool
    // On release: proxy success/failure is automatically recorded
    
    handle.release().await;
    Ok(())
}
```

When a browser is released after use, the proxy's circuit breaker is updated:

- **Clean return to idle queue**: proxy marked as success ✓
- **Browser unhealthy**: proxy marked as failure ✗  
- **Browser crashed**: proxy marked as failure ✗

---

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};
use stygian_browser::page::ResourceFilter;
use std::time::Duration;

# async fn run() -> stygian_browser::error::Result<()> {
let pool = BrowserPool::new(BrowserConfig::default()).await?;
let handle = pool.acquire().await?;
let mut page = handle.browser().unwrap().new_page().await?;

// Block images/fonts to speed up text-only scraping
page.set_resource_filter(ResourceFilter::block_media()).await?;

page.navigate(
    "https://example.com",
    WaitUntil::Selector("h1".to_string()),
    Duration::from_secs(30),
).await?;

// Evaluate JavaScript
let title: String = page.eval("document.title").await?;
let h1: String = page.eval("document.querySelector('h1')?.textContent ?? ''").await?;

// Full page HTML
let html = page.content().await?;

// Save cookies for session reuse
let cookies = page.save_cookies().await?;

page.close().await?;
handle.release().await;
# Ok(())
# }
```

---

## WebRTC & Proxy

```rust,no_run
use stygian_browser::{BrowserConfig};
use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy, ProxyLocation};

let config = BrowserConfig::builder()
    .proxy("http://proxy.example.com:8080".to_string())
    .webrtc(WebRtcConfig {
        policy: WebRtcPolicy::DisableNonProxied,
        location: Some(ProxyLocation::new_us_east()),
        ..Default::default()
    })
    .build();
```

`WebRtcPolicy::BlockAll` is the safest option for anonymous scraping — it prevents
any IP addresses from leaking via WebRTC peer connections.

---

## FAQ

**Q: Does this work on macOS / Linux / Windows?**  
A: macOS and Linux are fully supported.  Windows support depends on the `chromiumoxide`
backend; not actively tested.

**Q: Which Chrome versions are supported?**  
A: The library targets Chrome 120+.  Older versions may work but stealth scripts are
only tested against current release channels.

**Q: Can I use it without a display (CI/CD)?**  
A: Yes — the default config is `headless: true`.  No display server is required.

**Q: Does Advanced stealth guarantee Cloudflare bypass?**  
A: There is no guarantee.  Cloudflare Turnstile and Bot Management use both
JavaScript signals and TLS/network-layer heuristics.  Advanced stealth eliminates
all known JavaScript signals, which is necessary but may not be sufficient.

**Q: How do I set a custom Chrome path?**  
A: Set `STYGIAN_CHROME_PATH=/path/to/chrome` or use
`BrowserConfig::builder().chrome_path("/path/to/chrome".into()).build()`.

**Q: Why does `stats().idle` always return 0?**  
A: `idle` is a lock-free approximation.  The count is not maintained in the hot
acquire/release path to avoid contention.  Use `available` and `active` instead.

**Q: Should I set `STYGIAN_DISABLE_SANDBOX=true`?**  
A: Only inside a container (Docker, Kubernetes, etc.) where Chromium's renderer
sandbox cannot function due to missing user namespaces.  This is auto-detected via
`/.dockerenv` and `/proc/1/cgroup` on Linux — you normally don't need to set it
explicitly.  **Never set this on a bare-metal host** without an equivalent isolation
boundary; doing so removes a meaningful OS-level security layer.

For highest-security deployments, run each browser session in its own container and
let the container runtime provide isolation — the sandbox flag will be set
automatically inside the container.

---

## Testing

```bash
# Pure-logic unit tests (no Chrome required)
cargo test --lib -p stygian-browser

# Integration tests (requires Chrome 120+)
cargo test --all-features -p stygian-browser

# Run only ignored Chrome tests explicitly
cargo test --all-features -p stygian-browser -- --include-ignored

# Measure coverage for logic units
cargo tarpaulin -p stygian-browser --lib --ignore-tests --out Lcov
```

**Coverage notes**: All tests that launch a real browser instance are annotated
`#[ignore = "requires Chrome"]` so the suite passes in CI without a Chrome binary.
Pure-logic coverage (config, stealth scripts, fingerprint generation, simulator math)
is high; overall line coverage is structurally bounded by the CDP requirement.

---

## License

Licensed under the [GNU Affero General Public License v3.0](../../LICENSE) (`AGPL-3.0-only`).
