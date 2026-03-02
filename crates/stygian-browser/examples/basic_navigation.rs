//! Example 1: Basic navigation
//!
//! Demonstrates launching a browser, navigating to a URL, and extracting
//! the page title and HTML content.
//!
//! ```sh
//! cargo run --example basic_navigation -p stygian-browser
//! ```

use std::time::Duration;

use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialise structured logging (optional)
    tracing_subscriber::fmt::init();

    // Build config — defaults to headless Chrome with Advanced stealth
    let config = BrowserConfig::default();

    // Launch a warm pool of browsers
    println!("Launching browser pool...");
    let pool = BrowserPool::new(config).await?;
    println!("Pool ready — stats: {:?}", pool.stats());

    // Acquire a browser handle from the warm pool
    let handle = pool.acquire().await?;

    // Open a new tab
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    // Navigate and wait until <body> is present
    println!("Navigating to https://example.com ...");
    page.navigate(
        "https://example.com",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    // Extract data
    let title = page.title().await?;
    let html = page.content().await?;

    println!("Title  : {title}");
    println!("HTML   : {} bytes", html.len());
    println!(
        "Snippet: {}",
        html.chars()
            .take(200)
            .collect::<String>()
            .replace('\n', " ")
    );

    // Explicitly close the tab (also happens on drop)
    page.close().await?;

    // Return the browser to the pool
    handle.release().await;

    println!("Done.");
    Ok(())
}
