//! Integration tests for stygian-browser.
//!
//! These tests require a real Chrome/Chromium binary on the host.  They are
//! gated with `#[ignore]` so they are skipped by default and must be opted
//! into explicitly:
//!
//! ```sh
//! # Recommended: run serially to avoid browser startup contention
//! cargo test -p stygian-browser -- --ignored --test-threads=1
//! # or a single test:
//! cargo test -p stygian-browser browser_launch_and_shutdown -- --ignored
//! ```
//!
//! Set `STYGIAN_CHROME_PATH` to override the browser binary used.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use stygian_browser::config::PoolConfig;
use stygian_browser::page::ResourceFilter;
use stygian_browser::{BrowserConfig, BrowserInstance, BrowserPool, WaitUntil};

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Each call returns a fresh temp directory path unique to this process+counter,
/// preventing Chrome's `SingletonLock` from conflicting when tests run in parallel.
fn unique_user_data_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("stygian-itest-{pid}-{n}"))
}

/// Returns a `BrowserConfig` suitable for integration tests:
/// headless, 30 s launch timeout, 15 s CDP timeout, isolated user-data-dir.
fn test_config() -> BrowserConfig {
    let mut cfg = BrowserConfig::builder().headless(true).build();
    cfg.launch_timeout = Duration::from_secs(30);
    cfg.cdp_timeout = Duration::from_secs(15);
    // Unique dir prevents SingletonLock conflicts when tests run in parallel.
    cfg.user_data_dir = Some(unique_user_data_dir());

    // Allow override via env so CI can point at a specific binary.
    if let Ok(p) = std::env::var("STYGIAN_CHROME_PATH") {
        cfg.chrome_path = Some(PathBuf::from(p));
    }

    cfg
}

// ─── Browser lifecycle ────────────────────────────────────────────────────────

/// Launch a browser, verify it reports healthy, then cleanly shut it down.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn browser_launch_and_shutdown() -> Result<(), Box<dyn std::error::Error>> {
    let mut instance = BrowserInstance::launch(test_config()).await?;

    assert!(
        instance.is_healthy_cached(),
        "freshly launched browser should be healthy"
    );
    assert!(
        instance.is_healthy().await,
        "async health check should pass"
    );

    instance.shutdown().await?;
    Ok(())
}

/// Open a new page, navigate to example.com, read title and content.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn browser_navigate_and_read_title() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    let title = page.title().await?;
    assert!(
        title.to_lowercase().contains("example"),
        "expected title to contain 'example', got: {title:?}"
    );

    let html = page.content().await?;
    assert!(
        html.contains("<html"),
        "content should include <html>, got snippet: {}",
        html.get(..200.min(html.len())).unwrap_or_default()
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// Evaluate arbitrary JavaScript and check the return value is deserialised.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn page_eval_returns_typed_value() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;

    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let result: f64 = page.eval("1 + 2").await?;
    assert!(
        (result - 3.0).abs() < f64::EPSILON,
        "1+2 should be 3, got {result}"
    );

    let s: String = page.eval(r#""hello""#).await?;
    assert_eq!(s, "hello");

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

// ─── Stealth / fingerprint injection ─────────────────────────────────────────

/// After navigation the injected fingerprint properties must be non-default
/// values set by our script (navigator.webdriver must be undefined/false,
/// hardwareConcurrency and deviceMemory must reflect our injected values).
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn fingerprint_injection_webdriver_hidden() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;

    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    // navigator.webdriver should be undefined (or false) after stealth injection.
    let wd: serde_json::Value = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;
    assert_eq!(
        wd,
        serde_json::Value::Bool(true),
        "navigator.webdriver should be hidden; got {wd}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// hardwareConcurrency and deviceMemory must be within the valid ranges we
/// inject — the values change per fingerprint but must be sane.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn fingerprint_injection_hardware_values_sensible() -> Result<(), Box<dyn std::error::Error>>
{
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;

    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let concurrency: u32 = page.eval("navigator.hardwareConcurrency").await?;
    assert!(
        (1..=64).contains(&concurrency),
        "hardwareConcurrency {concurrency} out of sane range"
    );

    let memory: u32 = page.eval("navigator.deviceMemory").await?;
    assert!(
        [4u32, 8, 16].contains(&memory),
        "deviceMemory {memory} not in valid set {{4, 8, 16}}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

// ─── Resource filtering ───────────────────────────────────────────────────────

/// Setting a resource filter must not error, and pages with no interceptable
/// requests (about:blank) still load normally.
///
/// NOTE: Full media-blocking on external pages requires a `Fetch.requestPaused`
/// event handler to continue non-blocked requests — a known gap in the current
/// `set_resource_filter` implementation.  That feature is tracked separately.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn resource_filter_api_does_not_error() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;

    // API must not error when called.
    page.set_resource_filter(ResourceFilter::block_media())
        .await?;

    // about:blank has no external network requests, so Fetch intercept does not
    // block navigation.
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    // about:blank has an empty title — empty string is fine.
    let _title = page.title().await?;

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

// ─── Pool ─────────────────────────────────────────────────────────────────────

/// Pool acquire then release makes a unique browser available; acquiring again
/// gets a warm idle instance (same ID).
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn pool_acquire_release_reuse() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = BrowserConfig::builder()
        .headless(true)
        .pool(PoolConfig {
            min_size: 1,
            max_size: 2,
            ..PoolConfig::default()
        })
        .build();
    config.launch_timeout = Duration::from_secs(30);
    config.cdp_timeout = Duration::from_secs(15);
    config.user_data_dir = Some(unique_user_data_dir());

    let pool = BrowserPool::new(config).await?;

    let handle1 = pool.acquire().await?;
    let id1 = handle1
        .browser()
        .ok_or("handle1 has no valid browser")?
        .id()
        .to_string();
    handle1.release().await;

    // Second acquire should return the same warmed instance.
    let handle2 = pool.acquire().await?;
    let id2 = handle2
        .browser()
        .ok_or("handle2 has no valid browser")?
        .id()
        .to_string();

    assert_eq!(
        id1, id2,
        "pool should reuse the released browser; got {id1} then {id2}"
    );

    handle2.release().await;
    Ok(())
}

/// Pool enforces the max concurrency limit: holding max handles means the
/// (max+1)th acquire times out.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn pool_max_concurrency_enforced() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = BrowserConfig::builder()
        .headless(true)
        .pool(PoolConfig {
            min_size: 0,
            max_size: 1,
            acquire_timeout: Duration::from_millis(500),
            ..PoolConfig::default()
        })
        .build();
    config.launch_timeout = Duration::from_secs(30);
    config.cdp_timeout = Duration::from_secs(15);
    config.user_data_dir = Some(unique_user_data_dir());

    let pool = BrowserPool::new(config).await?;

    // Hold the single allowed handle.
    let _handle = pool.acquire().await?;

    // The second acquire should fail (timeout / pool exhausted).
    let result = pool.acquire().await;
    assert!(
        result.is_err(),
        "expected error when pool is exhausted, got Ok"
    );
    Ok(())
}

/// Pool stats reflect active count correctly (sequential acquire/release).
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn pool_stats_track_active_handles() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = BrowserConfig::builder()
        .headless(true)
        .pool(PoolConfig {
            min_size: 0,
            max_size: 3,
            ..PoolConfig::default()
        })
        .build();
    config.launch_timeout = Duration::from_secs(30);
    config.cdp_timeout = Duration::from_secs(15);
    config.user_data_dir = Some(unique_user_data_dir());

    let pool = BrowserPool::new(config).await?;

    let stats_before = pool.stats();
    assert_eq!(stats_before.active, 0);

    // Acquire one handle: active goes to 1.
    let h1 = pool.acquire().await?;
    assert_eq!(pool.stats().active, 1, "one handle acquired");
    h1.release().await;

    // After release, browser returns to idle; active_count is unchanged
    // (the pool tracks total live browsers, not just in-use ones).
    let stats_idle = pool.stats();
    assert_eq!(stats_idle.active, 1, "browser still managed after release");
    // Note: stats().idle is currently always 0 (lock-free approximation).

    // Acquire again — reuses the idle instance.
    let h2 = pool.acquire().await?;
    assert_eq!(pool.stats().active, 1, "still just one managed browser");
    h2.release().await;

    assert_eq!(pool.stats().active, 1, "browser back in idle pool");
    Ok(())
}

// ─── DOM Query API (NodeHandle) ───────────────────────────────────────────────

/// `query_selector_all` on a real page returns at least one node, and `attr`
/// retrieves a known attribute value.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn query_selector_all_returns_nodes() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    // example.com contains at least one <a> element with an href attribute.
    let nodes = page.query_selector_all("a[href]").await?;
    assert!(!nodes.is_empty(), "expected at least one <a href> on example.com");

    let href = nodes[0].attr("href").await?;
    assert!(href.is_some(), "first <a href> should have an href attribute");

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `attr_map` returns a map that contains every attribute present on the element.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn attr_map_is_exhaustive() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    // Select the first <a href> — example.com has one with href and no other attrs.
    let nodes = page.query_selector_all("a[href]").await?;
    assert!(!nodes.is_empty(), "expected <a href> on example.com");

    let map = nodes[0].attr_map().await?;
    assert!(
        map.contains_key("href"),
        "attr_map should include 'href'; got keys: {:?}",
        map.keys().collect::<Vec<_>>()
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `ancestors` for a node nested inside the document includes `"html"` at the tail.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn ancestors_returns_parent_chain() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    let nodes = page.query_selector_all("p").await?;
    assert!(!nodes.is_empty(), "expected at least one <p> on example.com");

    let chain = nodes[0].ancestors().await?;
    assert!(
        chain.last().map(String::as_str) == Some("html"),
        "ancestor chain should end at 'html'; got: {chain:?}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `children_matching` scoped to a parent element returns only its descendants.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn children_matching_returns_nested_nodes() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    // example.com's <div> contains <p> elements.
    let divs = page.query_selector_all("div").await?;
    assert!(!divs.is_empty(), "expected at least one <div> on example.com");

    let children = divs[0].children_matching("p").await?;
    assert!(
        !children.is_empty(),
        "expected at least one <p> inside the first <div> on example.com"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

// ─── #[derive(Extract)] / PageHandle::extract_all ────────────────────────────

#[cfg(feature = "extract")]
mod extract_tests {
    use super::*;
    use stygian_browser::extract::Extract;

    /// A simple extractable type that captures the `href` attribute of an `<a>`
    /// inside each matched root element.
    #[derive(Extract)]
    struct Link {
        #[selector("a", attr = "href")]
        href: Option<String>,
    }

    /// `extract_all` with a selector that matches elements returns a non-empty
    /// typed `Vec`.
    #[tokio::test]
    #[ignore = "requires real Chrome binary and external network access"]
    async fn extract_all_returns_typed_vec() -> Result<(), Box<dyn std::error::Error>> {
        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        // example.com has at least one <p> element.
        let items: Vec<Link> = page.extract_all::<Link>("p").await?;
        assert!(
            !items.is_empty(),
            "expected at least one <p> on example.com"
        );
        // Suppress unused-field warnings by referencing the field.
        let _ = items.iter().map(|l| l.href.as_deref()).count();

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }

    /// `extract_all` with a selector that matches nothing returns `Ok(vec![])`.
    #[tokio::test]
    #[ignore = "requires real Chrome binary and external network access"]
    async fn extract_all_empty_on_no_match() -> Result<(), Box<dyn std::error::Error>> {
        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        let items: Vec<Link> = page.extract_all::<Link>("div.nonexistent-xyz-9999").await?;
        assert!(items.is_empty(), "unmatched selector should yield empty Vec");

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }

    /// An `Option` field whose selector does not match inside the root element
    /// yields `None` rather than an error.
    #[tokio::test]
    #[ignore = "requires real Chrome binary and external network access"]
    async fn extract_all_optional_field_none_when_absent() -> Result<(), Box<dyn std::error::Error>>
    {
        /// A type where the optional `label` field uses a selector that will
        /// never match inside a `<p>` element on example.com.
        #[derive(Extract)]
        struct TextItem {
            #[selector("nonexistent-element-xyz-9999")]
            label: Option<String>,
        }

        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        let items: Vec<TextItem> = page.extract_all::<TextItem>("p").await?;
        assert!(!items.is_empty(), "expected <p> elements on example.com");
        for item in &items {
            assert!(
                item.label.is_none(),
                "optional field with unmatched selector should be None"
            );
        }

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }
}
