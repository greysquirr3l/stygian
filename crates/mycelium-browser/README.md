# mycelium-browser

High-performance, anti-detection browser automation library for Rust.

Built on the [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/) via
[`chromiumoxide`](https://github.com/mattsse/chromiumoxide) with comprehensive stealth features
for bypassing modern anti-bot systems: Cloudflare, DataDome, PerimeterX, Akamai.

---

## Features

| Feature | Description |
| --------- | ------------- |
| **Browser pooling** | Warm pool with configurable min/max, LRU eviction, backpressure |
| **Anti-detection** | Navigator spoofing, canvas noise, WebGL randomisation, UA patching |
| **Human behavior** | Bézier-curve mouse paths, realistic keystroke timing, random interactions |
| **CDP leak protection** | Hides `Runtime.enable` artifacts that expose automation |
| **WebRTC control** | Block, proxy-route, or allow WebRTC — prevent IP leaks |
| **Fingerprint generation** | Statistically-weighted device profiles (Windows, Mac, Linux, Android, iOS) |
| **Stealth levels** | `None` / `Basic` / `Advanced` — tune evasion vs. performance |

---

## Installation

```toml
[dependencies]
mycelium-browser = { path = "../crates/mycelium-browser" }   # workspace
# or once published to crates.io:
# mycelium-browser = "0.1"
tokio = { version = "1", features = ["full"] }
```

Enable (or disable) stealth features:

```toml
[dependencies]
# stealth is the default feature; disable for a minimal build
mycelium-browser = { version = "0.1", default-features = false }
```

---

## Quick Start

```rust,no_run
use mycelium_browser::{BrowserConfig, BrowserPool, WaitUntil};
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
    let mut page = handle.browser().new_page().await?;
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
use mycelium_browser::{BrowserConfig, StealthLevel};
use mycelium_browser::config::PoolConfig;
use mycelium_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
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
| `MYCELIUM_CHROME_PATH` | auto-detect | Path to Chrome/Chromium binary |
| `MYCELIUM_HEADLESS` | `true` | `false` for headed mode |
| `MYCELIUM_STEALTH_LEVEL` | `advanced` | `none`, `basic`, `advanced` |
| `MYCELIUM_POOL_MIN` | `2` | Minimum warm browser count |
| `MYCELIUM_POOL_MAX` | `10` | Maximum concurrent browsers |
| `MYCELIUM_POOL_ACQUIRE_TIMEOUT_SECS` | `30` | Seconds to wait for pool slot |
| `MYCELIUM_CDP_FIX_MODE` | `addBinding` | `addBinding`, `isolatedworld`, `enabledisable` |
| `MYCELIUM_PROXY` | — | Proxy URL |

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
use mycelium_browser::{BrowserConfig, BrowserPool};
use mycelium_browser::config::PoolConfig;
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

The Chrome DevTools Protocol itself can expose automation.  Three modes are
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

## Page Operations

```rust,no_run
use mycelium_browser::{BrowserConfig, BrowserPool, WaitUntil};
use mycelium_browser::page::ResourceFilter;
use std::time::Duration;

# async fn run() -> mycelium_browser::error::Result<()> {
let pool = BrowserPool::new(BrowserConfig::default()).await?;
let handle = pool.acquire().await?;
let mut page = handle.browser().new_page().await?;

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
use mycelium_browser::{BrowserConfig};
use mycelium_browser::webrtc::{WebRtcConfig, WebRtcPolicy, ProxyLocation};

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
A: Set `MYCELIUM_CHROME_PATH=/path/to/chrome` or use
`BrowserConfig::builder().chrome_path("/path/to/chrome".into()).build()`.

**Q: Why does `stats().idle` always return 0?**  
A: `idle` is a lock-free approximation.  The count is not maintained in the hot
acquire/release path to avoid contention.  Use `available` and `active` instead.

---

## License

MIT OR Apache-2.0
