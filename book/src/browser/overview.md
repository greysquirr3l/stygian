# Browser Automation Overview

`stygian-browser` is a high-performance, anti-detection browser automation library for Rust.
It is built on the [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
via [`chromiumoxide`](https://github.com/mattsse/chromiumoxide) and ships a comprehensive suite
of stealth features for bypassing modern bot-detection systems.

---

## Feature summary

| Feature | Description |
| --- | --- |
| **Browser pooling** | Warm pool with configurable min/max, LRU eviction, backpressure, and per-context segregation |
| **Anti-detection** | `navigator` spoofing, canvas noise, WebGL randomisation, UA patching |
| **Headless mode** | `HeadlessMode::New` default (`--headless=new`) — shares Chrome's headed rendering pipeline, harder to fingerprint-detect |
| **Human behaviour** | Bézier-curve mouse paths, realistic keystroke timing, typo simulation |
| **CDP leak protection** | Hides `Runtime.enable` artifacts that expose automation |
| **WebRTC control** | Block, proxy-route, or allow — prevents IP leaks |
| **Fingerprint generation** | Statistically-weighted device profiles (Windows, Mac, Linux, mobile) |
| **Stealth levels** | `None` / `Basic` / `Advanced` — tune evasion vs. performance |
| **Resource filtering** | Block images, fonts, media per-tab to speed up text scraping |
| **Cookie persistence** | Save/restore full session state (cookies + localStorage); `inject_cookies()` for seeding individual tokens |
| **Live DOM query** | `query_selector_all()` returns typed `NodeHandle` values; no full-HTML serialisation round-trip |
| **DOM traversal** | `NodeHandle::parent()`, `next_sibling()`, `previous_sibling()` for element-level tree walking |
| **Similarity matching** | `find_similar()` locates structurally equivalent elements across page versions using weighted Jaccard scoring (`similarity` feature) |
| **Structured extraction** | `#[derive(Extract)]` proc-macro maps CSS selectors directly onto Rust structs (`stygian-extract-derive`) |

---

## Use cases

| Scenario | Recommended config |
| --- | --- |
| Public HTML scraping (no bot detection) | `StealthLevel::None`, HTTP adapter preferred |
| Single-page app rendering | `StealthLevel::Basic`, `WaitUntil::NetworkIdle` |
| Cloudflare / DataDome protected sites | `StealthLevel::Advanced` |
| Price monitoring (authenticated sessions) | `StealthLevel::Basic` + cookie persistence |
| CAPTCHA-adjacent flows | `StealthLevel::Advanced` + human behaviour + proxy |

---

## Quick start

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Default config: headless, Advanced stealth, 2 warm browsers
    let pool   = BrowserPool::new(BrowserConfig::default()).await?;
    let handle = pool.acquire().await?;            // < 100 ms from warm pool

    let browser = handle
        .browser()
        .ok_or_else(|| std::io::Error::other("browser handle already released"))?;
    let mut page = browser.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    ).await?;

    println!("Title: {}", page.title().await?);
    println!("HTML:  {} bytes", page.content().await?.len());

    handle.release().await;               // returns browser to pool
    Ok(())
}
```

---

## Performance targets

| Operation | Target |
| --- | --- |
| Browser acquisition (warm pool) | < 100 ms |
| Browser launch (cold start) | < 2 s |
| Advanced stealth injection overhead | 10–30 ms per page |
| Pool health check | < 5 ms |

---

## Platform support

| Platform | Status |
| --- | --- |
| macOS (Apple Silicon / Intel) | Fully supported, actively tested |
| Linux (x86-64, ARM64) | Fully supported, CI tested |
| Windows | Supported via CI matrix on `windows-latest`; backend behavior depends on `chromiumoxide` |
| Headless CI (GitHub Actions) | Supported — default config is `headless: true` |

---

## Installation

```toml
[dependencies]
stygian-browser = "*"
tokio            = { version = "1", features = ["full"] }
```

To disable stealth features for a minimal build:

```toml
stygian-browser = { version = "*", default-features = false }
```

Chrome 120+ must be available on the system or specified via `STYGIAN_CHROME_PATH`.
On CI, install it with `apt-get install google-chrome-stable` or use the
`browser-actions/setup-chrome` GitHub Action.
