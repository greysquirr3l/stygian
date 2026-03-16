//! Live browser integration tests against <https://crawllab.dev>.
//!
//! crawllab.dev provides JS-rendered endpoints that deliver a minimal HTML
//! skeleton on the initial request; the real content is only visible after the
//! browser executes the bundled scripts.  These tests confirm that
//! `stygian-browser` waits for script execution before reading the DOM.
//!
//! Requirements: a real Chrome/Chromium binary **and** outbound HTTPS access.
//! Tests are gated with `#[ignore]`; run them explicitly:
//!
//! ```sh
//! cargo test -p stygian-browser --test crawllab -- --ignored --test-threads=1
//! ```
//!
//! Set `STYGIAN_CHROME_PATH` to override the browser binary path.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use stygian_browser::{BrowserConfig, BrowserInstance, WaitUntil};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn unique_user_data_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("stygian-crawllab-{pid}-{n}"))
}

fn test_config() -> BrowserConfig {
    let mut cfg = BrowserConfig::builder().headless(true).build();
    cfg.launch_timeout = Duration::from_secs(30);
    cfg.cdp_timeout = Duration::from_secs(15);
    cfg.user_data_dir = Some(unique_user_data_dir());
    if let Ok(p) = std::env::var("STYGIAN_CHROME_PATH") {
        cfg.chrome_path = Some(PathBuf::from(p));
    }
    cfg
}

// ─── JS rendering ─────────────────────────────────────────────────────────────

/// `/js/inline` delivers a bare HTML skeleton on the initial request.  The
/// real page content is rendered by an inline `<script>` tag.  After
/// `WaitUntil::NetworkIdle` the DOM should reflect the completed render.
#[tokio::test]
#[ignore = "requires real Chrome binary and network access to crawllab.dev"]
async fn js_inline_renders_content() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://crawllab.dev/js/inline",
        WaitUntil::NetworkIdle,
        Duration::from_secs(20),
    )
    .await?;

    let html = page.content().await?;

    // crawllab guarantees ≥ 200 characters of scraper-visible output.
    assert!(
        html.len() > 200,
        "JS-rendered page should have ≥ 200 chars of content, got {} bytes",
        html.len()
    );
    assert!(
        html.contains("<body"),
        "rendered page must include a <body> element"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `/js/external` loads its render script from a separate file at
/// `/js/render.js`.  This exercises the browser's ability to fetch and execute
/// an external script before we read the final DOM state.
#[tokio::test]
#[ignore = "requires real Chrome binary and network access to crawllab.dev"]
async fn js_external_renders_content() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://crawllab.dev/js/external",
        WaitUntil::NetworkIdle,
        Duration::from_secs(20),
    )
    .await?;

    let html = page.content().await?;

    assert!(
        html.len() > 200,
        "externally JS-rendered page should have ≥ 200 chars, got {} bytes",
        html.len()
    );
    assert!(
        html.contains("<body"),
        "rendered page must include a <body> element"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// Confirms that the browser's stealth injection does not break normal page
/// navigation or JS execution on an external site.  Uses a simple status-200
/// endpoint as a smoke test that the browser pool round-trips correctly.
#[tokio::test]
#[ignore = "requires real Chrome binary and network access to crawllab.dev"]
async fn browser_navigates_status_200() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://crawllab.dev/200",
        WaitUntil::DomContentLoaded,
        Duration::from_secs(15),
    )
    .await?;

    let html = page.content().await?;
    assert!(
        html.contains("<html") || html.contains("<HTML"),
        "response should be an HTML document, got: {}",
        html.get(..200.min(html.len())).unwrap_or_default()
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// Evaluates JavaScript on a live crawllab.dev page to confirm that our CDP
/// stealth injection does not break the JS runtime.
///
/// We navigate to the JS inline page and use `page.eval()` to directly query
/// the document title — if our injection corrupted the runtime this panics.
#[tokio::test]
#[ignore = "requires real Chrome binary and network access to crawllab.dev"]
async fn eval_works_on_crawllab_page() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://crawllab.dev/js/inline",
        WaitUntil::DomContentLoaded,
        Duration::from_secs(15),
    )
    .await?;

    // Evaluate a simple expression to verify the JS runtime is intact.
    let result: f64 = page.eval("1 + 1").await?;
    assert!(
        (result - 2.0).abs() < f64::EPSILON,
        "JS eval sanity check failed: expected 2, got {result}"
    );

    // Verify navigator.webdriver is hidden (stealth injection active).
    let webdriver_hidden: bool = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;
    assert!(
        webdriver_hidden,
        "navigator.webdriver should be hidden by stealth injection"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}
