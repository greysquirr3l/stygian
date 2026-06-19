#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn
)]
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
    assert!(
        !nodes.is_empty(),
        "expected at least one <a href> on example.com"
    );

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected first <a href> node"))?;
    let href = first.attr("href").await?;
    assert!(
        href.is_some(),
        "first <a href> should have an href attribute"
    );

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

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected first <a href> node"))?;
    let map = first.attr_map().await?;
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
    assert!(
        !nodes.is_empty(),
        "expected at least one <p> on example.com"
    );

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected first <p> node"))?;
    let chain = first.ancestors().await?;
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
    assert!(
        !divs.is_empty(),
        "expected at least one <div> on example.com"
    );

    let first_div = divs
        .first()
        .ok_or_else(|| std::io::Error::other("expected first <div> node"))?;
    let children = first_div.children_matching("p").await?;
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
        let href_count = items.iter().filter(|l| l.href.is_some()).count();
        assert!(
            href_count <= items.len(),
            "sanity check for extracted href count"
        );

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
        assert!(
            items.is_empty(),
            "unmatched selector should yield empty Vec"
        );

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

// ─── DOM Traversal API (T32) ──────────────────────────────────────────────────

/// Helper: navigate `page` to an inline HTML string via a `data:` URL.
///
/// The HTML is base64-encoded to avoid quoting issues in the URL.
fn data_url(html: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in html.as_bytes() {
        let _ = write!(encoded, "{byte:02x}");
    }
    // Use percent-encoded UTF-8 for the data URL to keep it simple;
    // base64 would require a dep, so we use a verbatim approach:
    // Chrome accepts `data:text/html,<escaped>` reliably.
    format!(
        "data:text/html,{}",
        html.chars().fold(String::new(), |mut acc, c| {
            if c.is_ascii_alphanumeric() || "<>/=\"' \n\r\t;:.#{}[]()!-_".contains(c) {
                acc.push(c);
            } else {
                let _ = write!(acc, "%{:02X}", c as u32);
            }
            acc
        })
    )
}

/// `parent()` returns the containing element.
///
/// DOM: `<ul><li id="first">A</li><li>B</li></ul>`
/// Select `#first`, call `.parent()`, assert `outer_html` contains `<ul`.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn parent_returns_node() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"<html><body><ul><li id="first">A</li><li>B</li></ul></body></html>"#;
    page.navigate(
        &data_url(html),
        WaitUntil::Selector("#first".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let nodes = page.query_selector_all("#first").await?;
    assert!(!nodes.is_empty(), "expected #first element");

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected #first node"))?;
    let parent = first.parent().await?;
    assert!(parent.is_some(), "parent() of <li> should be Some");

    let parent_node = parent.ok_or_else(|| std::io::Error::other("expected parent node"))?;
    let outer = parent_node.outer_html().await?;
    assert!(
        outer.contains("<ul") || outer.contains("<UL"),
        "parent of <li> should be <ul>; got: {outer}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `outer_html()` should include deeply nested descendants used by mesh-style
/// page builders (for example, Wix Studio / Editor X wrappers).
#[tokio::test]
#[ignore = "requires Chrome"]
async fn outer_html_includes_deep_mesh_descendants() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"
<html><body>
    <section data-block-level-container="ClassicSection">
        <div data-mesh-id="mesh-container-1">
            <div data-mesh-id="mesh-container-2">
                <div data-mesh-id="mesh-container-3">
                    <div class="wixui-rich-text" data-testid="richTextElement">
                        <p>Mesh content here</p>
                    </div>
                </div>
            </div>
        </div>
    </section>
</body></html>
"#;

    page.navigate(
        &data_url(html),
        WaitUntil::Selector("[data-block-level-container=\"ClassicSection\"]".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let sections = page
        .query_selector_all("[data-block-level-container=\"ClassicSection\"]")
        .await?;
    assert!(!sections.is_empty(), "expected at least one section");

    let section = sections
        .first()
        .ok_or_else(|| std::io::Error::other("expected section node"))?;

    let outer = section.outer_html().await?;
    assert!(
        outer.contains("data-mesh-id=\"mesh-container-3\""),
        "outer_html should include deep mesh descendants; got: {outer}"
    );
    assert!(
        outer.contains("Mesh content here"),
        "outer_html should include deep text content; got: {outer}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `outer_html_with_strategy(Current)` returns the same content as the
/// historical `outer_html()` wrapper for a simple page.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn outer_html_strategy_current_matches_default() -> Result<(), Box<dyn std::error::Error>> {
    use stygian_browser::page::OuterHtmlStrategy;

    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"<html><body><article id="x"><p>hello</p></article></body></html>"#;
    page.navigate(
        &data_url(html),
        WaitUntil::Selector("#x".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let nodes = page.query_selector_all("#x").await?;
    let node = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected #x node"))?;

    let legacy = node.outer_html().await?;
    let result = node
        .outer_html_with_strategy(OuterHtmlStrategy::Current)
        .await?;
    let content = result
        .content()
        .ok_or_else(|| std::io::Error::other("Current strategy should yield Content"))?;

    assert_eq!(
        legacy, content,
        "outer_html() and outer_html_with_strategy(Current) must agree on Content"
    );
    assert!(
        content.contains("<article"),
        "Current strategy must include opening tag; got: {content}"
    );
    assert!(
        content.contains("hello"),
        "Current strategy must include inner text; got: {content}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `outer_html_with_strategy(Recursive)` uses CDP `DOM.getOuterHTML` and
/// returns the full outer markup including deeply nested descendants on the
/// same Wix-style mesh page that the legacy call already covers.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn outer_html_strategy_recursive_includes_deep_mesh() -> Result<(), Box<dyn std::error::Error>>
{
    use stygian_browser::page::OuterHtmlStrategy;

    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"
<html><body>
    <section data-block-level-container="ClassicSection">
        <div data-mesh-id="mesh-container-1">
            <div data-mesh-id="mesh-container-2">
                <div data-mesh-id="mesh-container-3">
                    <div class="wixui-rich-text" data-testid="richTextElement">
                        <p>Recursive mesh content</p>
                    </div>
                </div>
            </div>
        </div>
    </section>
</body></html>
"#;

    page.navigate(
        &data_url(html),
        WaitUntil::Selector("[data-block-level-container=\"ClassicSection\"]".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let sections = page
        .query_selector_all("[data-block-level-container=\"ClassicSection\"]")
        .await?;
    let section = sections
        .first()
        .ok_or_else(|| std::io::Error::other("expected section node"))?;

    let result = section
        .outer_html_with_strategy(OuterHtmlStrategy::Recursive)
        .await?;
    let content = result
        .content()
        .ok_or_else(|| std::io::Error::other("Recursive strategy should yield Content"))?;

    assert!(
        content.contains("data-mesh-id=\"mesh-container-3\""),
        "Recursive strategy must include deep mesh descendants; got: {content}"
    );
    assert!(
        content.contains("Recursive mesh content"),
        "Recursive strategy must include deep text content; got: {content}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `outer_html_with_strategy(Recursive)` on a shadow-DOM host should
/// surface the shadow content (CDP `DOM.getOuterHTML` includes shadow roots
/// by default).
#[tokio::test]
#[ignore = "requires Chrome"]
async fn outer_html_strategy_recursive_includes_shadow_dom()
-> Result<(), Box<dyn std::error::Error>> {
    use stygian_browser::page::OuterHtmlStrategy;

    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"
<html><body>
    <div id="host"></div>
    <script>
      const host = document.getElementById('host');
      const shadow = host.attachShadow({mode: 'open'});
      const span = document.createElement('span');
      span.id = 'shadow-content';
      span.textContent = 'shadow-text';
      shadow.appendChild(span);
    </script>
</body></html>
"#;
    page.navigate(
        &data_url(html),
        WaitUntil::Selector("#host".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let hosts = page.query_selector_all("#host").await?;
    let host = hosts
        .first()
        .ok_or_else(|| std::io::Error::other("expected #host node"))?;

    let result = host
        .outer_html_with_strategy(OuterHtmlStrategy::Recursive)
        .await?;
    let content = result
        .content()
        .ok_or_else(|| std::io::Error::other("Recursive strategy should yield Content"))?;

    assert!(
        content.contains("shadow-text"),
        "Recursive strategy must surface shadow-DOM content; got: {content}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `next_sibling()` advances to the next element in the same parent.
///
/// DOM: `<ul><li id="a">A</li><li id="b">B</li></ul>`
/// Select `#a`, call `.next_sibling()`, assert result is Some and has text "B".
#[tokio::test]
#[ignore = "requires Chrome"]
async fn next_sibling_advances() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"<html><body><ul><li id="a">A</li><li id="b">B</li></ul></body></html>"#;
    page.navigate(
        &data_url(html),
        WaitUntil::Selector("#a".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let nodes = page.query_selector_all("#a").await?;
    assert!(!nodes.is_empty(), "expected #a element");

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected #a node"))?;
    let next = first.next_sibling().await?;
    assert!(
        next.is_some(),
        "next_sibling() of first <li> should be Some"
    );

    let next_node = next.ok_or_else(|| std::io::Error::other("expected next sibling"))?;
    let text = next_node.text_content().await?;
    assert_eq!(text.trim(), "B", "next sibling should have text 'B'");

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `previous_sibling()` of the first child returns `None`.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn previous_sibling_of_first_is_none() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = r#"<html><body><ul><li id="first">A</li><li>B</li></ul></body></html>"#;
    page.navigate(
        &data_url(html),
        WaitUntil::Selector("#first".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let nodes = page.query_selector_all("#first").await?;
    assert!(!nodes.is_empty(), "expected #first element");

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected #first node"))?;
    let prev = first.previous_sibling().await?;
    assert!(
        prev.is_none(),
        "previous_sibling() of first child should be None"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `parent()` of `<html>` (root element) returns `None`.
#[tokio::test]
#[ignore = "requires Chrome"]
async fn parent_of_root_is_none() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    page.navigate(
        "about:blank",
        WaitUntil::Selector("html".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let nodes = page.query_selector_all("html").await?;
    assert!(!nodes.is_empty(), "expected <html> element");

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected <html> node"))?;
    let parent = first.parent().await?;
    assert!(
        parent.is_none(),
        "parent() of <html> should be None (no parentElement)"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// Lateral extraction: select a `<td>` by its text, traverse to its sibling,
/// and read the sibling's text — the motivating use-case for T32.
///
/// DOM: `<table><tr><td>Price</td><td>$9.99</td></tr></table>`
#[tokio::test]
#[ignore = "requires Chrome"]
async fn sibling_chain_lateral_extraction() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;
    let mut page = instance.new_page().await?;

    let html = concat!(
        "<html><body>",
        "<table><tr>",
        "<td id='label'>Price</td>",
        "<td id='value'>$9.99</td>",
        "</tr></table>",
        "</body></html>"
    );
    page.navigate(
        &data_url(html),
        WaitUntil::Selector("#label".to_string()),
        Duration::from_secs(15),
    )
    .await?;

    let nodes = page.query_selector_all("#label").await?;
    assert!(!nodes.is_empty(), "expected #label <td>");

    let first = nodes
        .first()
        .ok_or_else(|| std::io::Error::other("expected #label node"))?;
    let value_cell = first.next_sibling().await?;
    assert!(
        value_cell.is_some(),
        "next sibling of label cell should be Some"
    );

    let value_node =
        value_cell.ok_or_else(|| std::io::Error::other("expected value sibling cell"))?;
    let price = value_node.text_content().await?;
    assert_eq!(
        price.trim(),
        "$9.99",
        "lateral extraction should yield the price cell's text"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

// ─── Similarity API (T33) ─────────────────────────────────────────────────────

#[cfg(feature = "similarity")]
mod similarity_tests {
    use super::*;
    use stygian_browser::similarity::SimilarityConfig;

    /// `find_similar` with threshold 0.0 returns at least one result — the page
    /// always contains at least one element.
    #[tokio::test]
    #[ignore = "requires Chrome"]
    async fn find_similar_returns_same_element() -> Result<(), Box<dyn std::error::Error>> {
        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        // Grab a reference node.
        let refs = page.query_selector_all("p").await?;
        assert!(!refs.is_empty(), "expected at least one <p> on example.com");

        // With threshold 0.0 every element is a match — result must be non-empty.
        let result = page
            .find_similar(
                refs.first()
                    .ok_or_else(|| std::io::Error::other("expected reference <p> node"))?,
                SimilarityConfig {
                    threshold: 0.0,
                    max_results: 50,
                },
            )
            .await?;

        assert!(
            !result.is_empty(),
            "find_similar with threshold 0.0 should return at least one match"
        );

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }

    /// `find_similar` with threshold `1.1` (above the maximum score) must
    /// return an empty result set.
    #[tokio::test]
    #[ignore = "requires Chrome"]
    async fn find_similar_threshold_filters() -> Result<(), Box<dyn std::error::Error>> {
        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        let refs = page.query_selector_all("p").await?;
        assert!(!refs.is_empty(), "expected at least one <p> on example.com");

        // Threshold 1.1 exceeds the maximum possible score — must yield nothing.
        let result = page
            .find_similar(
                refs.first()
                    .ok_or_else(|| std::io::Error::other("expected reference <p> node"))?,
                SimilarityConfig {
                    threshold: 1.1,
                    max_results: 10,
                },
            )
            .await?;

        assert!(
            result.is_empty(),
            "threshold > 1.0 should filter all candidates; got {} results",
            result.len()
        );

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }

    /// On example.com, using a `<p>` reference with a moderate threshold should
    /// find at least one similar element (the page has multiple `<p>` tags with
    /// identical structure).
    #[tokio::test]
    #[ignore = "requires Chrome"]
    async fn find_similar_finds_peer_elements() -> Result<(), Box<dyn std::error::Error>> {
        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        let refs = page.query_selector_all("p").await?;
        assert!(!refs.is_empty(), "expected at least one <p> on example.com");

        // Threshold 0.5 is low enough to find structurally similar <p> elements.
        let result = page
            .find_similar(
                refs.first()
                    .ok_or_else(|| std::io::Error::other("expected reference <p> node"))?,
                SimilarityConfig {
                    threshold: 0.5,
                    max_results: 20,
                },
            )
            .await?;

        assert!(
            !result.is_empty(),
            "expected at least one similar element above threshold 0.5"
        );

        // Results should be ordered score-descending.
        for window in result.windows(2) {
            let [left, right] = window else {
                continue;
            };
            assert!(
                left.score >= right.score,
                "results must be ordered by score descending; got {:.3} then {:.3}",
                left.score,
                right.score
            );
        }

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }

    /// `NodeHandle::fingerprint()` returns an `ElementFingerprint` whose `tag`
    /// is a known lower-case HTML tag name.
    #[tokio::test]
    #[ignore = "requires Chrome"]
    async fn fingerprint_captures_tag_and_classes() -> Result<(), Box<dyn std::error::Error>> {
        let instance = BrowserInstance::launch(test_config()).await?;
        let mut page = instance.new_page().await?;

        page.navigate(
            "https://example.com",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(30),
        )
        .await?;

        let nodes = page.query_selector_all("p").await?;
        assert!(
            !nodes.is_empty(),
            "expected at least one <p> on example.com"
        );

        let first = nodes
            .first()
            .ok_or_else(|| std::io::Error::other("expected first <p> node"))?;
        let fp = first.fingerprint().await?;
        assert_eq!(fp.tag, "p", "fingerprint tag should be 'p'");

        // Classes and attr_names must be sorted (they may be empty on example.com).
        let mut sorted_classes = fp.classes.clone();
        sorted_classes.sort();
        assert_eq!(fp.classes, sorted_classes, "classes must be sorted");

        let mut sorted_attrs = fp.attr_names.clone();
        sorted_attrs.sort();
        assert_eq!(fp.attr_names, sorted_attrs, "attr_names must be sorted");

        page.close().await?;
        instance.shutdown().await?;
        Ok(())
    }
}

// ─── Freshness contracts (T79) ────────────────────────────────────────────────

/// End-to-end freshness rejection path:
/// build a stale contract, hand it to the runner via
/// `AcquisitionRequest::freshness_contract`, and assert that the
/// runner rejects the request without contacting the network and
/// reports a structured `stale_ttl` decision.
///
/// This exercises the integration of the freshness module into the
/// `acquisition-runner` feature path on a live browser pool.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-browser --test integration \
///     freshness_runner_rejects_stale_session -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn freshness_runner_rejects_stale_session() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use stygian_browser::freshness::{FreshnessContract, FreshnessPolicyKind};
    use stygian_browser::{AcquisitionMode, AcquisitionRequest, AcquisitionRunner};

    let pool = BrowserPool::new(test_config()).await?;
    let runner = AcquisitionRunner::new(pool);

    // Build a contract that has clearly expired (1 ms TTL, captured an
    // hour ago). The runner must reject before touching the browser.
    let captured_ms = stygian_browser::freshness::unix_epoch_ms().saturating_sub(60 * 60 * 1_000);
    let stale = FreshnessContract::with_signature(
        "example.com",
        "sha256:test-signature",
        captured_ms,
        Duration::from_millis(1),
        FreshnessPolicyKind::Strict,
    )?;

    let request = AcquisitionRequest {
        url: "https://example.com/".to_string(),
        mode: AcquisitionMode::Fast,
        total_timeout: Duration::from_secs(10),
        freshness_contract: Some(stale),
        ..AcquisitionRequest::default()
    };

    let result = runner.run(request).await;

    assert!(!result.success, "stale contract must not succeed");
    let report = result
        .freshness
        .as_ref()
        .ok_or_else(|| std::io::Error::other("freshness report missing"))?;
    assert!(report.decision.is_invalid(), "decision must be invalid");
    assert_eq!(
        report.decision.label(),
        "stale_ttl",
        "expected stale_ttl decision"
    );
    let reason = report
        .decision
        .reason()
        .ok_or_else(|| std::io::Error::other("reason missing"))?;
    assert_eq!(reason.contract_domain, "example.com");
    assert_eq!(
        reason.contract_signature.as_deref(),
        Some("sha256:test-signature")
    );
    assert!(reason.elapsed_ms > reason.max_age_ms);
    assert_eq!(result.attempted.len(), 0, "no stages should be attempted");
    assert_eq!(result.failures.len(), 1);
    assert_eq!(
        result.failures.first().map(|f| f.kind),
        Some(stygian_browser::StageFailureKind::Setup)
    );

    Ok(())
}

// ─── Cross-context coherence probes (T80) ─────────────────────────────────────

/// End-to-end coherence probe path: navigate to a real page, run
/// `CoherenceProbe::run`, and assert the report contains
/// per-context identity surfaces plus (best-effort) drift
/// diagnostics.
///
/// This exercises the cross-context probe integration on a live
/// browser pool. The probe is **default-on**, so no feature gate
/// is required. Worker probes are best-effort: when the runtime
/// does not support `Worker`, the worker slot is reported as
/// `Skipped` and the test still passes.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-browser --test integration \
///     coherence_probe_emits_per_context_report -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn coherence_probe_emits_per_context_report() -> Result<(), Box<dyn std::error::Error>> {
    use stygian_browser::coherence::CoherenceProbe;

    let pool = BrowserPool::new(test_config()).await?;
    let handle = pool.acquire().await?;
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    page.navigate(
        "https://example.com",
        WaitUntil::DomContentLoaded,
        Duration::from_secs(30),
    )
    .await?;

    let probe = CoherenceProbe::new();
    let report = probe.run(&page).await?;

    // Top-level + iframe contexts must be observed on a live browser.
    assert!(
        report.top.is_observed(),
        "top-level context must be observed, got: {:?}",
        report.top
    );
    assert!(
        report.iframe.is_observed(),
        "iframe context must be observed, got: {:?}",
        report.iframe
    );
    // Worker context is best-effort: observed when supported,
    // skipped otherwise. Either is acceptable.
    if let Some(surface) = report.worker.surface() {
        // When observed, user_agent MUST be present (every browser
        // exposes navigator.userAgent in a worker).
        assert!(
            surface.user_agent.is_some(),
            "worker observation must include user_agent"
        );
    }
    assert!(
        report.observed_context_count() >= 2,
        "expected at least top + iframe observed, got: {}/3",
        report.observed_context_count()
    );

    // Top ↔ iframe should be coherent on a clean browser (no UA,
    // platform, or languages drift).
    let top_iframe_drifts: Vec<&stygian_browser::coherence::DriftDiagnostic> = report
        .drifts
        .iter()
        .filter(|d| {
            d.context_a == stygian_browser::coherence::ContextKind::Top
                && d.context_b == stygian_browser::coherence::ContextKind::Iframe
        })
        .collect();
    let top_iframe_hard: Vec<&&stygian_browser::coherence::DriftDiagnostic> = top_iframe_drifts
        .iter()
        .filter(|d| d.severity == stygian_browser::coherence::DriftSeverity::Hard)
        .collect();
    assert!(
        top_iframe_hard.is_empty(),
        "no hard drift should exist between top and iframe, got: {top_iframe_hard:?}"
    );

    // Sanity: the report is JSON-serialisable.
    let _ = serde_json::to_string(&report)?;

    page.close().await?;
    handle.release().await;
    Ok(())
}

// ─── Replay defense (T81) ─────────────────────────────────────────────────────

/// End-to-end replay-defense forced refresh path: drive a live
/// browser through the `AcquisitionRunner` with a stale
/// `ReplayDefenseContext` and confirm the runner
/// 1. short-circuits with `ReplayDefenseTriggered`,
/// 2. attaches a `ReplayDefenseReport` to the result, and
/// 3. invalidates the sticky pool slots for the target host
///    via `BrowserPool::release_context`.
///
/// The test uses a 1 s `rotation_interval` so a context captured
/// 1 hour ago triggers a `RotationDue` decision — deterministic,
/// no real signature drift required.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-browser --test integration \
///     replay_defense_forces_refresh_on_signature_drift -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn replay_defense_forces_refresh_on_signature_drift() -> Result<(), Box<dyn std::error::Error>>
{
    use std::time::Duration;
    use stygian_browser::replay_defense::{ReplayDefensePolicy, ReplayDefenseState};
    use stygian_browser::{AcquisitionMode, AcquisitionRequest, AcquisitionRunner};

    let pool = BrowserPool::new(test_config()).await?;
    let runner = AcquisitionRunner::new(pool);

    // Capture the state 1 hour in the past with a 1 second
    // rotation interval so the policy's `RotationDue` check fires
    // immediately. The state carries a `sha256:` signature that the
    // runner observes, so signature drift is also detected.
    let captured_ms =
        stygian_browser::replay_defense::unix_epoch_ms().saturating_sub(60 * 60 * 1_000);
    let state = ReplayDefenseState::with_fingerprint(
        "example.com",
        "sha256:test-signature",
        Some("nonce-001"),
        captured_ms,
    );
    let policy = ReplayDefensePolicy {
        rotation_interval: Duration::from_secs(1),
        nonce_validity_window: Duration::from_secs(1),
        force_reset_on_drift: true,
    };
    let context = stygian_browser::ReplayDefenseContext::with_policy(policy, state);

    let request = AcquisitionRequest {
        url: "https://example.com/".to_string(),
        mode: AcquisitionMode::Fast,
        total_timeout: Duration::from_secs(10),
        replay_defense: Some(context),
        ..AcquisitionRequest::default()
    };

    let result = runner.run(request).await;

    assert!(!result.success, "replay defense must reject the run");
    let report = result
        .replay_defense
        .as_ref()
        .ok_or_else(|| std::io::Error::other("replay defense report missing"))?;
    assert!(report.decision.is_invalid(), "decision must be invalid");
    assert!(
        report.forced_refresh,
        "forced_refresh flag must be set, got: {report:?}"
    );
    // Either RotationDue or NonceExpired is acceptable here because
    // both elapsed (1 hour) and nonce age (1 hour) exceed the 1 s
    // windows.
    let label = report.decision.label();
    assert!(
        label == "rotation_due" || label == "nonce_expired",
        "expected rotation_due or nonce_expired, got: {label}"
    );
    assert_eq!(result.attempted.len(), 0, "no stages should be attempted");
    assert_eq!(result.failures.len(), 1);
    assert_eq!(
        result.failures.first().map(|f| f.kind),
        Some(stygian_browser::StageFailureKind::ReplayDefenseTriggered)
    );

    Ok(())
}

// ─── Transport realism (T82) ──────────────────────────────────────────────

/// End-to-end transport-realism path: drive a live browser through
/// the `AcquisitionRunner` with a [`TransportRealismContext`] and
/// confirm the runner attaches a [`TransportRealismReport`] to the
/// result that includes the documented additive JSON sections.
///
/// The test uses the Chrome 136 reference observation so the score
/// resolves to a strong match without needing a real TLS capture.
/// This proves the `AcquisitionResult::transport_realism` field is
/// populated and the diagnostic payload (when forwarded through the
/// [`DiagnosticReport::with_transport_realism`] builder) includes
/// the new `transport_realism` section in a backward-compatible
/// way.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-browser --test integration \
///     transport_realism_section_appears_in_diagnostic_payload -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn transport_realism_section_appears_in_diagnostic_payload()
-> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use stygian_browser::diagnostic::DiagnosticReport;
    use stygian_browser::transport_realism::{TransportObservation, TransportProfile};
    use stygian_browser::{
        AcquisitionMode, AcquisitionRequest, AcquisitionRunner, TransportRealismContext,
    };

    let pool = BrowserPool::new(test_config()).await?;
    let runner = AcquisitionRunner::new(pool);

    // Chrome 136 reference observation → strong match, well-covered.
    let observation = TransportObservation::chrome_136_reference();
    let profile = TransportProfile::default();
    let context = TransportRealismContext::with_observation(profile, observation);

    let request = AcquisitionRequest {
        url: "https://example.com/".to_string(),
        mode: AcquisitionMode::Fast,
        total_timeout: Duration::from_secs(15),
        transport_realism: Some(context),
        ..AcquisitionRequest::default()
    };

    let result = runner.run(request).await;

    // The runner attaches a transport_realism report on every run
    // where the context is supplied — success or failure.
    let realism_report = result
        .transport_realism
        .as_ref()
        .ok_or_else(|| std::io::Error::other("transport realism report missing"))?;
    assert_eq!(realism_report.profile_name, "chrome-136");
    assert!(
        realism_report.compatibility.score > 0.5,
        "chrome 136 reference observation should score high, got: {}",
        realism_report.compatibility.score
    );
    assert!(
        realism_report.compatibility.is_high_confidence(),
        "coverage should be high, got: {:?}",
        realism_report.compatibility
    );
    assert_eq!(realism_report.compatibility.total_checks, 3);

    // DiagnosticReport::with_transport_realism wires the new section
    // in. The JSON must contain the new `transport_realism` key but
    // must NOT rename any pre-existing field — this is the
    // backward-compatible contract.
    let diagnostic =
        DiagnosticReport::new(Vec::new()).with_transport_realism(realism_report.clone());
    let json = serde_json::to_string(&diagnostic)?;
    assert!(
        json.contains("\"transport_realism\""),
        "diagnostic JSON must include transport_realism section, got: {json}"
    );
    // Pre-existing fields are preserved.
    assert!(
        json.contains("\"checks\""),
        "diagnostic JSON must preserve pre-existing checks field"
    );
    assert!(
        json.contains("\"passed_count\""),
        "diagnostic JSON must preserve pre-existing passed_count field"
    );
    // Round-trip: deserialize back and verify the section survives.
    let restored: DiagnosticReport = serde_json::from_str(&json)?;
    let restored_realism = restored
        .transport_realism
        .as_ref()
        .ok_or_else(|| std::io::Error::other("restored transport realism missing"))?;
    assert_eq!(restored_realism.profile_name, "chrome-136");
    #[allow(clippy::float_cmp)]
    let _ = (
        restored_realism.compatibility.score,
        realism_report.compatibility.score,
    );
    assert!(
        (restored_realism.compatibility.score - realism_report.compatibility.score).abs() < 1e-9
    );

    Ok(())
}

// ─── JavaScript integrity trap canary (T92) ──────────────────────────────────

/// End-to-end integrity canary probe path: navigate to a real page,
/// run the built-in [`IntegrityProbe`][stygian_browser::integrity_canary::IntegrityProbe]
/// set via CDP `Runtime.evaluate`, parse each probe's JSON output
/// into a [`ProbeFinding`][stygian_browser::integrity_canary::ProbeFinding],
/// build an [`IntegrityCanaryReport`][stygian_browser::integrity_canary::IntegrityCanaryReport],
/// and assert the report includes trap findings + mitigation hints.
///
/// The probe set is **default-on**, so no feature gate is required.
/// On a clean browser the score is `0.0` (Clean classification) and
/// no mitigation hints are emitted — the test still asserts the
/// report carries the per-probe findings + score so downstream
/// automation sees a stable schema even on clean runs.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-browser --test integration \
///     integrity_canary_emits_trap_findings_and_hints -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
#[allow(clippy::too_many_lines)]
async fn integrity_canary_emits_trap_findings_and_hints() -> Result<(), Box<dyn std::error::Error>>
{
    use stygian_browser::diagnostic::DiagnosticReport;
    use stygian_browser::integrity_canary::{
        IntegrityCanaryReport, IntegrityProbeOutcome, IntegrityRiskClassification, all_probes,
    };

    let pool = BrowserPool::new(test_config()).await?;
    let handle = pool.acquire().await?;
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    page.navigate(
        "https://example.com",
        WaitUntil::DomContentLoaded,
        Duration::from_secs(30),
    )
    .await?;

    // Run each probe against the live page and collect findings.
    let mut findings = Vec::new();
    for probe in all_probes() {
        let raw: String = page.eval(probe.script).await?;
        findings.push(probe.parse_output(&raw));
    }

    // Build the report. Even on a clean browser the report must
    // carry the full finding set so the diagnostic payload schema
    // is stable across runs.
    let report = IntegrityCanaryReport::from_findings(findings.clone());
    assert_eq!(
        report.findings.len(),
        all_probes().len(),
        "report must carry one finding per probe in the catalogue"
    );

    // The aggregate score is in `[0.0, 1.0]` and the classification
    // is one of the three documented bands.
    assert!(
        (0.0..=1.0).contains(&report.score.value),
        "score must be in [0.0, 1.0], got: {}",
        report.score.value
    );
    let classification = report.score.classification;
    assert!(
        matches!(
            classification,
            IntegrityRiskClassification::Clean
                | IntegrityRiskClassification::Suspected
                | IntegrityRiskClassification::Confirmed
        ),
        "classification must be Clean / Suspected / Confirmed, got: {classification:?}"
    );

    // Trap findings + mitigation hints: on a clean browser there
    // are no traps so trap_findings and mitigation_hints are
    // empty. The schema contract is that they're empty Vecs (not
    // missing) so downstream automation can iterate them safely.
    let fired_clean_or_skipped = report.findings.iter().all(|f| {
        matches!(
            f.outcome,
            IntegrityProbeOutcome::Clean | IntegrityProbeOutcome::Skipped
        )
    });
    if fired_clean_or_skipped {
        assert!(
            report.trap_findings.is_empty(),
            "clean browser must produce no trap findings"
        );
        assert!(
            report.mitigation_hints.is_empty(),
            "clean browser must produce no mitigation hints"
        );
    } else {
        // If any trap fired, verify the mitigation hint schema
        // matches the documented contract.
        for hint in &report.mitigation_hints {
            assert!(
                !hint.probe_id.is_empty(),
                "mitigation hint must carry the probe id"
            );
            assert!(
                !hint.hint.is_empty(),
                "mitigation hint text must be non-empty"
            );
            assert!(
                matches!(
                    hint.outcome,
                    IntegrityProbeOutcome::TrapSuspected | IntegrityProbeOutcome::TrapConfirmed
                ),
                "mitigation hint must reference a fired trap"
            );
        }
    }

    // DiagnosticReport integration: the canary report attaches
    // additively without breaking the legacy schema.
    let diagnostic = DiagnosticReport::new(Vec::new()).with_integrity_canary(report.clone());
    let json = serde_json::to_string(&diagnostic)?;
    assert!(
        json.contains("\"integrity_canary\""),
        "diagnostic JSON must include integrity_canary section, got: {json}"
    );
    // Pre-existing fields are preserved.
    assert!(
        json.contains("\"checks\""),
        "diagnostic JSON must preserve pre-existing checks field"
    );
    assert!(
        json.contains("\"passed_count\""),
        "diagnostic JSON must preserve pre-existing passed_count field"
    );
    // Round-trip: deserialize back and verify the section survives.
    let restored: DiagnosticReport = serde_json::from_str(&json)?;
    let restored_canary = restored
        .integrity_canary
        .as_ref()
        .ok_or_else(|| std::io::Error::other("restored integrity_canary missing"))?;
    assert_eq!(
        restored_canary.score.classification, report.score.classification,
        "classification must survive JSON round-trip"
    );
    assert_eq!(
        restored_canary.findings.len(),
        report.findings.len(),
        "findings count must survive JSON round-trip"
    );

    Ok(())
}

// ─── Queue and interstitial detection routing (T94) ────────────────────────────

/// End-to-end interstitial routing path: drive a live
/// browser through the `AcquisitionRunner` with a
/// classified `InterstitialContext` and confirm the
/// runner
/// 1. attaches the `RouterDecision` to the result,
/// 2. short-circuits with `InterstitialRouted` for
///    classified states, and
/// 3. bypasses the generic ladder (no stages attempted).
///
/// The test attaches deterministic page signatures that
/// classify as `HardBlock` (403 + "access denied"
/// marker), `Queue` ("please wait" body marker), and
/// `Transient` (3xx redirect). The hard-block and queue
/// classifications must short-circuit; the transient one
/// must fall through.
///
/// Run with:
///
/// ```sh
/// cargo test -p stygian-browser --test integration \
///     interstitial_routing_short_circuits_runner -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn interstitial_routing_short_circuits_runner() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use stygian_browser::InterstitialContext;
    use stygian_browser::interstitial_router::PageSignature;
    use stygian_browser::{
        AcquisitionMode, AcquisitionRequest, AcquisitionRunner, StageFailureKind,
    };

    // Sub-test 1: hard-block classification must short-circuit.
    {
        let pool = BrowserPool::new(test_config()).await?;
        let runner = AcquisitionRunner::new(pool);
        let signature = PageSignature::new("https://example.com/blocked", Some(403))
            .with_body_marker("access denied");
        let context = InterstitialContext::new(signature);

        let request = AcquisitionRequest {
            url: "https://example.com/".to_string(),
            mode: AcquisitionMode::Fast,
            total_timeout: Duration::from_secs(10),
            interstitial: Some(context),
            ..AcquisitionRequest::default()
        };

        let result = runner.run(request).await;
        let decision = result
            .interstitial
            .as_ref()
            .ok_or_else(|| std::io::Error::other("interstitial decision missing"))?;
        assert_eq!(decision.kind().label(), "hard_block");
        assert!(decision.is_classified());
        assert_eq!(result.attempted.len(), 0, "no stages attempted");
        assert_eq!(result.failures.len(), 1);
        assert_eq!(
            result.failures.first().map(|f| f.kind),
            Some(StageFailureKind::InterstitialRouted)
        );
    }

    // Sub-test 2: queue classification also short-circuits.
    {
        let pool = BrowserPool::new(test_config()).await?;
        let runner = AcquisitionRunner::new(pool);
        let signature = PageSignature::new("https://example.com/queue", Some(200))
            .with_body_marker("please wait")
            .with_queue_position(5);
        let context = InterstitialContext::new(signature);

        let request = AcquisitionRequest {
            url: "https://example.com/".to_string(),
            mode: AcquisitionMode::Fast,
            total_timeout: Duration::from_secs(10),
            interstitial: Some(context),
            ..AcquisitionRequest::default()
        };

        let result = runner.run(request).await;
        let decision = result
            .interstitial
            .as_ref()
            .ok_or_else(|| std::io::Error::other("interstitial decision missing"))?;
        assert_eq!(decision.kind().label(), "queue");
        assert_eq!(
            result.failures.first().map(|f| f.kind),
            Some(StageFailureKind::InterstitialRouted)
        );
    }

    // Sub-test 3: transient classification does NOT
    // short-circuit; the runner falls through to the
    // ladder. The live pool will attempt the stages
    // (which may succeed or fail depending on the
    // network), but the interstitial decision is still
    // attached to the result.
    {
        let pool = BrowserPool::new(test_config()).await?;
        let runner = AcquisitionRunner::new(pool);
        let signature = PageSignature::new("https://example.com/redirect", Some(302));
        let context = InterstitialContext::new(signature);

        let request = AcquisitionRequest {
            url: "https://example.com/".to_string(),
            mode: AcquisitionMode::Fast,
            total_timeout: Duration::from_secs(15),
            interstitial: Some(context),
            ..AcquisitionRequest::default()
        };

        let result = runner.run(request).await;
        let decision = result
            .interstitial
            .as_ref()
            .ok_or_else(|| std::io::Error::other("interstitial decision missing"))?;
        assert_eq!(decision.kind().label(), "transient");
        assert!(!decision.is_classified());
        // The transient decision must NOT trigger a
        // short-circuit, so the runner may have attempted
        // some stages (or timed out). Either is acceptable;
        // what we strictly require is that no
        // InterstitialRouted failure is present.
        assert!(
            result
                .failures
                .iter()
                .all(|f| f.kind != StageFailureKind::InterstitialRouted),
            "transient interstitial must not short-circuit"
        );
    }

    Ok(())
}
