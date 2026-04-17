//! Example 8: CLI scraper with structured JSON output
//!
//! Accepts a URL on the command line, scrapes it using an Advanced-stealth
//! browser pool, and emits a structured JSON document to stdout.
//!
//! Extracted fields:
//!
//! - `url` / `final_url` (after redirects)
//! - `status_code`
//! - `title` and meta `description`
//! - `headings` — up to 10 `<h1>`, `<h2>`, `<h3>` elements with level+text
//! - `links` — up to 20 `<a href>` elements with resolved href + truncated text
//! - `text_excerpt` — first 800 chars of `document.body.innerText`
//! - `load_time_ms` — wall-clock time from request to `NetworkIdle`
//! - `scraped_at` — Unix epoch seconds
//!
//! Works well on both static pages and JS-heavy SPAs (fingerprint test pages,
//! React/Vue apps, etc.) because it waits for `NetworkIdle` before extracting.
//!
//! ```sh
//! cargo run --example scraper_cli -p stygian-browser -- https://example.com
//!
//! # Fingerprint detection test:
//! cargo run --example scraper_cli -p stygian-browser -- https://pixelscan.net/fingerprint-check
//!
//! # Pretty-print with jq:
//! cargo run --example scraper_cli -p stygian-browser -- https://example.com | jq .
//! ```

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use stygian_browser::config::{PoolConfig, StealthLevel};
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ─── main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── CLI arg ───────────────────────────────────────────────────────────────
    let url = std::env::args().nth(1).ok_or(
        "Usage: scraper_cli <URL>\n  e.g. cargo run --example scraper_cli -p stygian-browser -- https://example.com",
    )?;

    eprintln!("[scraper] target : {url}");

    // ── Browser pool ──────────────────────────────────────────────────────────
    // Keep the pool small for a one-shot CLI: 1 warm + 1 burst.
    let config = BrowserConfig::builder()
        .headless(true)
        .stealth_level(StealthLevel::Advanced)
        .pool(PoolConfig {
            min_size: 1,
            max_size: 2,
            idle_timeout: Duration::from_mins(1),
            acquire_timeout: Duration::from_secs(30),
        })
        .build();

    eprintln!("[scraper] warming browser pool...");
    let pool = BrowserPool::new(config).await?;

    let handle = pool.acquire().await?;
    let browser = handle
        .browser()
        .ok_or("browser pool returned an expired handle")?;
    let mut page = browser.new_page().await?;

    // Do NOT block resources on detection-test pages — font and image loading
    // behaviour is itself a fingerprint signal that scanners measure.

    // ── Navigate ──────────────────────────────────────────────────────────────
    eprintln!("[scraper] navigating...");
    let t0 = Instant::now();
    // NetworkIdle waits for all XHR/fetch activity to settle — essential for
    // SPAs and JS-driven fingerprint test pages (e.g. pixelscan.net) that
    // render results asynchronously after DOMContentLoaded.
    page.navigate(&url, WaitUntil::NetworkIdle, Duration::from_secs(45))
        .await?;
    let load_time_ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);

    eprintln!("[scraper] loaded in {load_time_ms}ms");

    // ── Extract structured data via CDP eval ──────────────────────────────────

    let final_url = page.url().await.unwrap_or_else(|_| url.clone());
    let title = page.title().await.unwrap_or_default();
    let status_code = page.status_code().unwrap_or(None).unwrap_or(0);

    // Meta description — try name="description" then og:description
    let description: String = page
        .eval(
            "document.querySelector('meta[name=\"description\"]')?.content \
             || document.querySelector('meta[property=\"og:description\"]')?.content \
             || ''",
        )
        .await
        .unwrap_or_default();

    // First 10 headings (h1, h2, h3) with their level tag and text
    let headings: Value = page
        .eval(
            "Array.from(document.querySelectorAll('h1,h2,h3')).slice(0, 10)\
              .map(h => ({ level: h.tagName.toLowerCase(), text: h.textContent.trim() }))",
        )
        .await
        .unwrap_or(json!([]));

    // First 20 external-looking links with resolved href and truncated anchor text
    let links: Value = page
        .eval(
            "Array.from(document.querySelectorAll('a[href]')).slice(0, 40)\
              .map(a => ({ href: a.href, text: a.textContent.trim().slice(0, 120) }))\
              .filter(l => l.href.startsWith('http'))\
              .slice(0, 20)",
        )
        .await
        .unwrap_or(json!([]));

    // First 800 chars of body text — covers rendered SPA content (test results, etc.)
    let text_excerpt: String = page
        .eval("(document.body?.innerText || '').trim().replace(/\\s+/g, ' ').slice(0, 800)")
        .await
        .unwrap_or_default();

    // ── Build output ──────────────────────────────────────────────────────────
    let result = json!({
        "url":          url,
        "final_url":    final_url,
        "status_code":  status_code,
        "title":        title,
        "description":  description,
        "headings":     headings,
        "links":        links,
        "text_excerpt": text_excerpt,
        "load_time_ms": load_time_ms,
        "scraped_at":   epoch_secs(),
    });

    // ── Emit to stdout ────────────────────────────────────────────────────────
    println!("{}", serde_json::to_string_pretty(&result)?);

    // ── Cleanup ───────────────────────────────────────────────────────────────
    page.close().await.ok();
    handle.release().await;

    eprintln!("[scraper] done.");
    Ok(())
}
