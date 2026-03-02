//! Example 5: Browser pool — concurrent acquire and release
//!
//! Demonstrates initialising a pool with custom sizes and acquiring/releasing
//! multiple browsers concurrently using `tokio::spawn`.
//!
//! ```sh
//! cargo run --example browser_pool -p stygian-browser
//! ```

use std::sync::Arc;
use std::time::Duration;

use stygian_browser::config::PoolConfig;
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Configure a pool of 2–4 browsers
    let config = BrowserConfig::builder()
        .headless(true)
        .pool(PoolConfig {
            min_size: 2,
            max_size: 4,
            idle_timeout: Duration::from_secs(120),
            acquire_timeout: Duration::from_secs(30),
        })
        .build();

    println!("Launching browser pool (min=2, max=4)...");
    let pool = Arc::new(BrowserPool::new(config).await?);

    let stats = pool.stats();
    println!(
        "Pool stats: available={}, max={}",
        stats.available, stats.max
    );

    // Spawn 3 concurrent scraping tasks
    let urls = vec![
        "https://example.com",
        "https://example.org",
        "https://example.net",
    ];

    let mut handles = Vec::new();

    for url in urls {
        let pool = Arc::clone(&pool);
        let url = url.to_string();

        let h = tokio::spawn(async move {
            // Acquire a browser slot (waiting up to acquire_timeout if pool is full)
            let browser_handle = pool.acquire().await.map_err(|e| e.to_string())?;
            let mut page = browser_handle
                .browser()
                .ok_or("browser handle no longer valid")?
                .new_page()
                .await
                .map_err(|e| e.to_string())?;

            page.navigate(
                &url,
                WaitUntil::Selector("body".to_string()),
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| e.to_string())?;

            let title = page.title().await.unwrap_or_default();
            page.close().await.map_err(|e| e.to_string())?;

            // Returning the handle to the pool
            browser_handle.release().await;

            Ok::<(String, String), String>((url, title))
        });

        handles.push(h);
    }

    // Collect results
    for handle in handles {
        match handle.await? {
            Ok((url, title)) => println!("  {url:30} => {title}"),
            Err(e) => eprintln!("  scrape failed: {e}"),
        }
    }

    println!("\nFinal pool stats: {:?}", pool.stats());
    println!("Done.");
    Ok(())
}
