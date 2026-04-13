//! Anti-detection validation test suite for stygian-browser.
//!
//! Validates that stealth features successfully evade known detection systems
//! and browser fingerprinting tools.  All tests require a real Chrome/Chromium
//! binary **and** external network access; they are gated with `#[ignore]`.
//!
//! # Running
//!
//! ```sh
//! # Run all detection tests serially (avoids browser startup contention)
//! cargo test -p stygian-browser --test detection -- --ignored --test-threads=1
//!
//! # Run a single test
//! cargo test -p stygian-browser --test detection stealth_webdriver_not_present -- --ignored
//! ```
//!
//! Set `STYGIAN_CHROME_PATH` to override the browser binary path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use stygian_browser::{BrowserConfig, BrowserInstance, WaitUntil};

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Returns a unique temp dir path per call, preventing Chrome's `SingletonLock`
/// from conflicting when multiple tests allocate browsers sequentially.
fn unique_user_data_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("stygian-detect-{pid}-{n}"))
}

/// Returns a `BrowserConfig` suitable for detection tests:
/// headless, unique user-data-dir, generous timeouts.
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

// ─── Property-level stealth checks ───────────────────────────────────────────
// These tests use `about:blank` and pure JS evals — they are fast and reliable.

/// `navigator.webdriver` must be undefined/false after stealth injection.
///
/// This is the #1 signal every major anti-bot system checks (`Cloudflare`,
/// `DataDome`, `PerimeterX`).  A truthy value means immediate bot detection.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_webdriver_not_present() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let hidden: bool = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;

    assert!(
        hidden,
        "navigator.webdriver should be hidden after stealth injection"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `navigator.plugins` must not be empty.
///
/// Real browsers expose at least a few plugin entries.  An empty `PluginArray`
/// is a reliable headless/automation indicator.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_plugins_not_empty() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let plugin_count: u32 = page.eval("navigator.plugins.length").await?;

    assert!(
        plugin_count > 0,
        "navigator.plugins should not be empty; got {plugin_count}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// The User-Agent must not contain `"HeadlessChrome"`.
///
/// The headless UA fingerprint is trivially detectable; stealth must replace it
/// with a plausible desktop UA that still contains `"Chrome/"`.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_user_agent_not_headless() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let ua: String = page.eval("navigator.userAgent").await?;

    assert!(
        !ua.contains("HeadlessChrome"),
        "User-Agent must not contain 'HeadlessChrome'; got: {ua}"
    );
    assert!(
        ua.contains("Chrome/"),
        "User-Agent should contain 'Chrome/'; got: {ua}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `window.chrome` must be defined.
///
/// Anti-bot systems verify the presence and structure of `window.chrome` as a
/// Chrome authenticity signal.  Headless Chrome omits this object by default.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_chrome_object_present() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let chrome_defined: bool = page.eval("typeof window.chrome !== 'undefined'").await?;

    assert!(
        chrome_defined,
        "window.chrome should be defined after stealth injection"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// CDP-injected automation properties (`$cdc_*`, `$chrome_asyncScriptInfo`) must
/// be absent from `window`.
///
/// `ChromeDriver` injects these globals and detection scripts check for them.
/// Our CDP protection mode should prevent their appearance.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_cdp_automation_properties_absent() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    // Scan all window keys for the two known ChromeDriver artifacts.
    let cdc_present: bool = page
        .eval(
            r"Object.keys(window).some(k =>
                k.startsWith('$cdc_') || k.startsWith('$chrome_asyncScript')
            )",
        )
        .await?;

    assert!(
        !cdc_present,
        "$cdc_* and $chrome_asyncScript* properties must be absent from window"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `navigator.permissions` must be present and functional.
///
/// Some anti-bot systems probe the Permissions API to distinguish real browsers
/// from headless environments that omit it.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_permissions_api_present() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let api_present: bool = page
        .eval("typeof navigator.permissions !== 'undefined' && typeof navigator.permissions.query === 'function'")
        .await?;

    assert!(
        api_present,
        "navigator.permissions should be present and expose query()"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// `navigator.language` and `navigator.vendor` must be non-empty strings.
///
/// Empty values are never present in real-user browsers and indicate an
/// improperly configured automation environment.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn stealth_language_and_vendor_not_empty() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    let language: String = page.eval("navigator.language || ''").await?;
    let vendor: String = page.eval("navigator.vendor || ''").await?;

    assert!(
        !language.is_empty(),
        "navigator.language should not be empty"
    );
    assert!(!vendor.is_empty(), "navigator.vendor should not be empty");

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

// ─── Real-world detection site checks ────────────────────────────────────────
// These require external network access.  They validate the same properties as
// above but via third-party detection pages to catch regressions early.

/// Navigate to `bot.sannysoft.com` and verify critical bot signals pass.
///
/// The site renders a table of automated bot-detection tests.  Regardless of
/// the page layout, we validate the critical navigator properties directly.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn sannysoft_critical_signals_pass() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "https://bot.sannysoft.com",
        WaitUntil::Selector("table".to_string()),
        Duration::from_secs(45),
    )
    .await?;

    let webdriver_hidden: bool = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;

    let ua: String = page.eval("navigator.userAgent").await?;

    let plugins: u32 = page.eval("navigator.plugins.length").await?;

    assert!(
        webdriver_hidden,
        "sannysoft: navigator.webdriver should be hidden"
    );
    assert!(
        !ua.contains("HeadlessChrome"),
        "sannysoft: UA should not be headless; got: {ua}"
    );
    assert!(
        plugins > 0,
        "sannysoft: navigator.plugins should not be empty; got {plugins}"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// Navigate to `browserleaks.com/javascript` and verify no automation signals.
///
/// `BrowserLeaks` displays navigator properties that fingerprinting scripts read.
/// Core properties must have plausible values (non-empty, non-headless).
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn browserleaks_no_automation_signals() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    page.navigate(
        "https://browserleaks.com/javascript",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    let webdriver_hidden: bool = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;

    let language: String = page.eval("navigator.language || ''").await?;

    let vendor: String = page.eval("navigator.vendor || ''").await?;

    assert!(
        webdriver_hidden,
        "browserleaks: navigator.webdriver must be hidden"
    );
    assert!(
        !language.is_empty(),
        "browserleaks: navigator.language must not be empty"
    );
    assert!(
        !vendor.is_empty(),
        "browserleaks: navigator.vendor must not be empty"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}

/// Navigate to `CreepJS` and confirm the page loads without a bot-crash.
///
/// `CreepJS` runs thorough fingerprinting; a blocked/error page would be
/// shorter than a few hundred bytes.  We also check the baseline properties.
#[tokio::test]
#[ignore = "requires real Chrome binary and external network access"]
async fn creepjs_page_loads_without_bot_crash() -> Result<(), Box<dyn std::error::Error>> {
    let instance = BrowserInstance::launch(test_config()).await?;

    let mut page = instance.new_page().await?;
    // CreepJS is JS-heavy; give it extra time.
    page.navigate(
        "https://abrahamjuliot.github.io/creepjs/",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(60),
    )
    .await?;

    let html = page.content().await?;
    assert!(
        html.len() > 500,
        "page content too short — may have been blocked (got {} bytes)",
        html.len()
    );

    // Baseline signal: webdriver must still be hidden on this page.
    let webdriver_hidden: bool = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;

    assert!(
        webdriver_hidden,
        "creepjs: navigator.webdriver must be hidden"
    );

    page.close().await?;
    instance.shutdown().await?;
    Ok(())
}
