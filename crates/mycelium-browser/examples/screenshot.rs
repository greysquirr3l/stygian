//! Example 3: Screenshot capture
//!
//! Navigates to a URL and saves a full-page PNG screenshot to disk.
//!
//! ```sh
//! cargo run --example screenshot -p mycelium-browser
//! # Output: screenshot.png in the current directory
//! ```

use std::time::Duration;

use mycelium_browser::{BrowserConfig, BrowserPool, WaitUntil};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = BrowserConfig::builder()
        .headless(true)
        .window_size(1920, 1080)
        .build();

    let pool = BrowserPool::new(config).await?;
    let handle = pool.acquire().await?;
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    let url = "https://example.com";
    println!("Navigating to {url} ...");

    page.navigate(
        url,
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    println!("Capturing screenshot...");
    let png_bytes = page.screenshot().await?;

    let out_path = "screenshot.png";
    std::fs::write(out_path, &png_bytes)?;

    println!(
        "Saved {out_path} ({} bytes / {} KiB)",
        png_bytes.len(),
        png_bytes.len() / 1024
    );

    page.close().await?;
    handle.release().await;

    println!("Done.");
    Ok(())
}
