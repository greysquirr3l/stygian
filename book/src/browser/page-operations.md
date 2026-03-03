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

// Wait for the DOM to be parsed (fast; before images/CSS load)
page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;

// Wait for network to go idle (good for SPAs — ≤ 2 in-flight requests for 500 ms)
page.navigate("https://example.com", WaitUntil::NetworkIdle, Duration::from_secs(30)).await?;
```

`WaitUntil` variants from fastest to safest:

| Variant | Condition |
|---|---|
| `DomContentLoaded` | HTML fully parsed; DOM ready (fires before images/stylesheets) |
| `NetworkIdle` | Load event fired **and** ≤ 2 in-flight requests for 500 ms |
| `Selector(css)` | `document.querySelector(selector)` returns a non-null element |

---

## Reading page content

```rust,no_run
// Full page HTML
let html  = page.content().await?;

// Page title
let title = page.title().await?;

// Current URL (may differ from navigated URL after redirects)
let url   = page.url().await?;

// HTTP status code of the last navigation, if available
// (None if no navigation has committed yet, the URL is non-HTTP such as file://,
//  or network events were not captured)
if let Some(status) = page.status_code()? {
    println!("HTTP {status}");
}
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

High-level click/type helpers are provided by the **human behaviour** module
(`stygian_browser::behavior`) when the `stealth` feature is enabled. These simulate
realistic mouse paths and typing cadence. See the [Stealth & Anti-Detection](stealth.md)
page for full usage.

```rust,no_run
use stygian_browser::behavior::{MouseSimulator, TypingSimulator};

let mouse = MouseSimulator::new();
mouse.move_to(&page, 100.0, 200.0, 450.0, 380.0).await?;
mouse.click(&page, 450.0, 380.0).await?;

let typer = TypingSimulator::new().wpm(90);
typer.type_into(&page, "#search-input", "rust async scraping").await?;

// For selector-based waiting without mouse simulation:
page.wait_for_selector(".results", Duration::from_secs(10)).await?;
```

---

## Screenshots

```rust,no_run
// Full-page screenshot — returns raw PNG bytes
let png: Vec<u8> = page.screenshot().await?;
tokio::fs::write("screenshot.png", &png).await?;
```

---

## Resource filtering

Block resource types to reduce bandwidth and speed up text-only scraping:

```rust,no_run
use stygian_browser::page::{ResourceFilter, ResourceType};

// Block images, fonts, CSS, and media
page.set_resource_filter(ResourceFilter::block_media()).await?;

// Block only images and fonts (keep styles for layout-sensitive work)
page.set_resource_filter(ResourceFilter::block_images_and_fonts()).await?;

// Custom filter
page.set_resource_filter(
    ResourceFilter::default()
        .block(ResourceType::Image)
        .block(ResourceType::Font)
        .block(ResourceType::Stylesheet)
).await?;
```

Must be called before `navigate()` to take effect.

---

## Cookie management

Session persistence is handled via the `session` module. Save and restore full session
state (cookies + localStorage) across runs, or inject individual cookies without a full
round-trip.

```rust,no_run
use stygian_browser::session::{save_session, restore_session, SessionSnapshot, SessionCookie};

// Save full session state after login
let snapshot: SessionSnapshot = save_session(&page).await?;
snapshot.save_to_file("session.json")?;

// Restore in a later run
let snapshot = SessionSnapshot::load_from_file("session.json")?;
restore_session(&page, &snapshot).await?;

// Inject individual cookies without a full snapshot (e.g. seed a known token)
let cookies = vec![SessionCookie {
    name:      "session".to_string(),
    value:     "abc123".to_string(),
    domain:    ".example.com".to_string(),
    path:      "/".to_string(),
    expires:   -1.0,   // session cookie
    http_only: true,
    secure:    true,
    same_site: "Lax".to_string(),
}];
page.inject_cookies(&cookies).await?;
```

Check whether a saved snapshot is still fresh before restoring:

```rust,no_run
let mut snapshot = SessionSnapshot::load_from_file("session.json")?;
snapshot.ttl_secs = Some(3600);   // 1-hour TTL
if snapshot.is_expired() {
    // re-authenticate
} else {
    restore_session(&page, &snapshot).await?;
}
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

    let url = page.url().await?;
    let status = page.status_code()?;
    match status {
        Some(code) => println!("{url} → HTTP {code}"),
        None => println!("{url} → HTTP status unknown"),
    }

    let html = page.content().await?;
    // … pass html to an AI extraction node …

    page.close().await?;
    handle.release().await;
    Ok(())
}
```
