//! Example 2: Form filling
//!
//! Demonstrates navigating to a page with a form, filling it in with
//! human-like typing, and reading the resulting content.
//!
//! Uses `httpbin.org/forms/post` as a stable test target.
//!
//! ```sh
//! cargo run --example form_filling -p mycelium-browser
//! ```

use std::time::Duration;

use mycelium_browser::{BrowserConfig, BrowserPool, WaitUntil};

#[cfg(feature = "stealth")]
use mycelium_browser::behavior::TypingSimulator;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = BrowserConfig::default();
    let pool = BrowserPool::new(config).await?;
    let handle = pool.acquire().await?;
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    println!("Navigating to form...");
    page.navigate(
        "https://httpbin.org/forms/post",
        WaitUntil::Selector("form".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    // Option A: direct JS-based fill (fast, works without "stealth" feature)
    println!("Filling form fields via JavaScript...");
    let _: serde_json::Value = page
        .eval(
            r"
            (function() {
                const custname = document.querySelector('[name=custname]');
                const custtel  = document.querySelector('[name=custtel]');
                const custemail = document.querySelector('[name=custemail]');
                if (custname)  custname.value  = 'Alice Example';
                if (custtel)   custtel.value   = '+1-555-0100';
                if (custemail) custemail.value  = 'alice@example.com';
                return { filled: true };
            })()
            ",
        )
        .await?;

    // Option B: human-like typing (only available when stealth feature is on)
    #[cfg(feature = "stealth")]
    {
        let raw_page = page.inner().clone();
        let mut typer = TypingSimulator::new();

        // Focus the size field and type a value
        let _: serde_json::Value = page
            .eval(r"document.querySelector('[name=size]')?.focus()")
            .await?;

        typer.type_text(&raw_page, "Large").await?;
        println!("Typed 'Large' with human-like timing");
    }

    // Submit the form
    println!("Submitting...");
    let _: serde_json::Value = page
        .eval("document.querySelector('[type=submit]')?.click()")
        .await?;

    // Wait for the result page
    page.navigate(
        "https://httpbin.org/forms/post",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or(()); // submission redirects; best-effort wait

    let html = page.content().await?;
    println!("Response body: {} bytes", html.len());

    page.close().await?;
    handle.release().await;

    println!("Done.");
    Ok(())
}
