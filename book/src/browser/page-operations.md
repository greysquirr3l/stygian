# Page Operations

A `Page` represents a single browser tab. You get one by calling `browser.new_page()` on a
`BrowserHandle`.

---

## Navigation

```rust,no_run
use stygian_browser::WaitUntil;
use std::time::Duration;

// Wait for a specific CSS selector to appear
page.navigate("https://example.com", WaitUntil::Selector("h1".into()), Duration::from_secs(30)).await?;

// Wait for the load event
page.navigate("https://example.com", WaitUntil::Load, Duration::from_secs(30)).await?;

// Wait for network to go idle (good for SPAs)
page.navigate("https://example.com", WaitUntil::NetworkIdle, Duration::from_secs(30)).await?;

// Bare navigation — returns as soon as the request is sent
page.navigate("https://example.com", WaitUntil::Commit, Duration::from_secs(10)).await?;
```

`WaitUntil` variants from fastest to safest:

| Variant | Condition |
|---|---|
| `Commit` | First bytes received |
| `Load` | `window.onload` fired |
| `NetworkIdle` | No network requests for 500 ms |
| `Selector(css)` | Element matching the selector is present in the DOM |

---

## Reading page content

```rust,no_run
// Full page HTML
let html  = page.content().await?;

// Page title
let title = page.title().await?;

// Current URL (may differ from navigated URL after redirects)
let url   = page.url().await?;
```

---

## JavaScript evaluation

```rust,no_run
// Evaluate an expression; return type must implement serde::DeserializeOwned
let title:   String = page.eval("document.title").await?;
let is_auth: bool   = page.eval("!!document.cookie.match(/session=/)").await?;
let item_count: u32 = page.eval("document.querySelectorAll('.item').length").await?;

// Execute a statement (return value ignored)
page.eval::<serde_json::Value>("window.scrollTo(0, document.body.scrollHeight)").await?;
```

---

## Element interaction

```rust,no_run
// Click an element by CSS selector
page.click("#submit-button").await?;

// Type into an input field
page.type_into("#search", "rust async scraping").await?;

// Select an option in a <select>
page.select("#country", "US").await?;

// Wait for a selector to appear (with timeout)
page.wait_for_selector(".results", Duration::from_secs(10)).await?;
```

---

## Screenshots

```rust,no_run
use stygian_browser::page::ScreenshotOptions;

// Full-page screenshot (PNG bytes)
let png = page.screenshot(ScreenshotOptions {
    full_page: true,
    format:    stygian_browser::page::ImageFormat::Png,
    ..Default::default()
}).await?;

tokio::fs::write("screenshot.png", &png).await?;
```

---

## Resource filtering

Block resource types to reduce bandwidth and speed up text-only scraping:

```rust,no_run
use stygian_browser::page::ResourceFilter;

// Block all images and fonts
page.set_resource_filter(ResourceFilter::block_media()).await?;

// Block everything except documents and scripts
page.set_resource_filter(ResourceFilter::documents_only()).await?;

// Custom filter
page.set_resource_filter(ResourceFilter::new()
    .block_type("image")
    .block_type("font")
    .block_type("stylesheet")).await?;
```

Must be called before `navigate()` to take effect.

---

## Cookie management

```rust,no_run
// Save all cookies for the current origin
let cookies = page.save_cookies().await?;

// Restore cookies in a new session
page.restore_cookies(&cookies).await?;
```

Serialize cookies with `serde_json` for persistent storage:

```rust,no_run
let json = serde_json::to_string(&cookies)?;
tokio::fs::write("cookies.json", &json).await?;

// Later...
let cookies: Vec<stygian_browser::Cookie> =
    serde_json::from_str(&tokio::fs::read_to_string("cookies.json").await?)?;
page.restore_cookies(&cookies).await?;
```

---

## Closing a tab

```rust,no_run
page.close().await?;
```

Always close pages explicitly when done. Unreleased pages count against the browser's
internal tab limit and may degrade performance.

---

## Complete example

```rust,no_run
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};
use stygian_browser::page::ResourceFilter;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool   = BrowserPool::new(BrowserConfig::default()).await?;
    let handle = pool.acquire().await?;
    let mut page = handle.browser().new_page().await?;

    // Block images to reduce bandwidth
    page.set_resource_filter(ResourceFilter::block_media()).await?;

    page.navigate(
        "https://example.com/products",
        WaitUntil::Selector(".product-list".to_string()),
        Duration::from_secs(30),
    ).await?;

    let count: u32 = page.eval("document.querySelectorAll('.product').length").await?;
    println!("{count} products found");

    let html = page.content().await?;
    // … pass html to an AI extraction node …

    page.close().await?;
    handle.release().await;
    Ok(())
}
```
