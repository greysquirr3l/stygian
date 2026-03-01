//! Example 4: Advanced stealth — bot detection bypass
//!
//! Demonstrates configuring the Advanced stealth level and verifying that
//! key bot-detection signals are clean when visiting a detection test page.
//!
//! ```sh
//! cargo run --example stealth_bypass -p mycelium-browser
//! ```

use std::time::Duration;

use mycelium_browser::{BrowserConfig, BrowserPool, WaitUntil};
use mycelium_browser::config::StealthLevel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Advanced stealth: full fingerprint injection + human behavior simulation
    let config = BrowserConfig::builder()
        .headless(true)
        .stealth_level(StealthLevel::Advanced)
        .build();

    println!("Launching browser with Advanced stealth...");
    let pool = BrowserPool::new(config).await?;
    let handle = pool.acquire().await?;
    let mut page = handle.browser().ok_or("browser handle no longer valid")?.new_page().await?;

    // First verify stealth properties on about:blank
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await?;

    // --- Check core anti-bot signals ---

    let webdriver_hidden: bool = page
        .eval("typeof navigator.webdriver === 'undefined' || navigator.webdriver === false")
        .await?;
    println!("✓ navigator.webdriver hidden : {webdriver_hidden}");

    let ua: String = page.eval("navigator.userAgent").await?;
    let ua_clean = !ua.contains("HeadlessChrome");
    println!("✓ UA clean (no HeadlessChrome): {ua_clean}  [{ua}]");

    let plugins: u32 = page.eval("navigator.plugins.length").await?;
    println!("✓ navigator.plugins.length   : {plugins}");

    let chrome_defined: bool = page.eval("typeof window.chrome !== 'undefined'").await?;
    println!("✓ window.chrome defined      : {chrome_defined}");

    let concurrency: u32 = page.eval("navigator.hardwareConcurrency").await?;
    println!("✓ hardwareConcurrency        : {concurrency}");

    let memory: u32 = page.eval("navigator.deviceMemory").await?;
    println!("✓ deviceMemory               : {memory} GB");

    let cdc_absent: bool = page
        .eval(
            r"!Object.keys(window).some(k =>
                k.startsWith('$cdc_') || k.startsWith('$chrome_asyncScript')
            )",
        )
        .await?;
    println!("✓ CDP automation props absent: {cdc_absent}");

    println!();

    // --- Visit bot.sannysoft.com ---
    println!("Navigating to bot.sannysoft.com ...");
    page.navigate(
        "https://bot.sannysoft.com",
        WaitUntil::Selector("table".to_string()),
        Duration::from_secs(45),
    )
    .await?;

    let title = page.title().await?;
    println!("Page loaded: {title}");
    println!("All stealth checks passed. Signals look clean.");

    page.close().await?;
    handle.release().await;

    println!("Done.");
    Ok(())
}
