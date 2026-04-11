//! Stealth regression canary probe.
//!
//! Runs [`verify_stealth`](stygian_browser::PageHandle::verify_stealth) against
//! every supplied URL with `Advanced` stealth enabled.  Outputs a JSON array to
//! stdout and exits with code 1 if any URL's score falls below the threshold.
//!
//! # Usage
//!
//! ```sh
//! cargo run --example stealth_probe --all-features -- [--threshold 0.90] <url>...
//! ```
//!
//! # Output
//!
//! A pretty-printed JSON array is written to stdout; one element per URL.
//! Exit code 0 = all pass, 1 = at least one URL below threshold, 2 = bad args.

use std::time::Duration;

use std::sync::Arc;

use serde_json::json;
use stygian_browser::config::StealthLevel;
use stygian_browser::diagnostic::TransportObservations;
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};

struct ProbeArgs {
    threshold: f64,
    urls: Vec<String>,
    observed_ja3_hash: Option<String>,
    observed_ja4: Option<String>,
    observed_http3_perk_text: Option<String>,
    observed_http3_perk_hash: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&raw_args)?;

    if args.urls.is_empty() {
        eprintln!(
            "usage: stealth_probe [--threshold 0.90] [--ja3-hash HASH] [--ja4 FP] [--http3-perk-text TEXT] [--http3-perk-hash HASH] <url>..."
        );
        std::process::exit(2);
    }

    let config = BrowserConfig::builder()
        .headless(true)
        .stealth_level(StealthLevel::Advanced)
        .build();

    let pool = BrowserPool::new(config).await?;

    let mut results = Vec::with_capacity(args.urls.len());
    let mut any_failed = false;

    for url in &args.urls {
        match probe_url(&pool, url, &args).await {
            Ok(entry) => {
                if !entry
                    .get("ok")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    any_failed = true;
                }
                results.push(entry);
            }
            Err(e) => {
                eprintln!("error probing {url}: {e}");
                results.push(json!({
                    "url": url,
                    "error": e.to_string(),
                    "ok": false,
                }));
                any_failed = true;
            }
        }
    }

    println!("{}", serde_json::to_string_pretty(&results)?);

    if any_failed {
        std::process::exit(1);
    }

    Ok(())
}

async fn probe_url(
    pool: &Arc<BrowserPool>,
    url: &str,
    args: &ProbeArgs,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let handle = pool.acquire().await?;
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    page.navigate(url, WaitUntil::DomContentLoaded, Duration::from_secs(20))
        .await?;

    let observations = TransportObservations {
        ja3_hash: args.observed_ja3_hash.clone(),
        ja4: args.observed_ja4.clone(),
        http3_perk_text: args.observed_http3_perk_text.clone(),
        http3_perk_hash: args.observed_http3_perk_hash.clone(),
    };
    let report = page
        .verify_stealth_with_transport(Some(observations))
        .await?;

    // coverage_pct() returns 0.0–100.0; normalise to 0.0–1.0 for threshold comparison.
    let score = report.coverage_pct() / 100.0;
    let ok = score >= args.threshold;

    let failed_checks: Vec<serde_json::Value> = report
        .failures()
        .map(|c| {
            json!({
                "id": c.id,
                "description": c.description,
                "details": c.details,
            })
        })
        .collect();

    let _ = page.close().await;
    handle.release().await;

    Ok(json!({
        "url": url,
        "score": score,
        "score_pct": report.coverage_pct(),
        "passed_count": report.passed_count,
        "failed_count": report.failed_count,
        "failed_checks": failed_checks,
        "threshold": args.threshold,
        "transport": report.transport,
        "ok": ok,
    }))
}

fn parse_args(args: &[String]) -> Result<ProbeArgs, Box<dyn std::error::Error>> {
    let mut threshold = 0.90_f64;
    let mut urls = Vec::new();
    let mut observed_ja3_hash = None;
    let mut observed_ja4 = None;
    let mut observed_http3_perk_text = None;
    let mut observed_http3_perk_hash = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if let Some(val) = arg.strip_prefix("--threshold=") {
            threshold = val
                .parse::<f64>()
                .map_err(|_| "--threshold must be a float in range 0.0–1.0")?;
        } else if arg == "--threshold" {
            let val = iter.next().ok_or("--threshold requires a value")?;
            threshold = val
                .parse::<f64>()
                .map_err(|_| "--threshold must be a float in range 0.0–1.0")?;
        } else if let Some(val) = arg.strip_prefix("--ja3-hash=") {
            observed_ja3_hash = Some(val.to_string());
        } else if arg == "--ja3-hash" {
            let val = iter.next().ok_or("--ja3-hash requires a value")?;
            observed_ja3_hash = Some(val.clone());
        } else if let Some(val) = arg.strip_prefix("--ja4=") {
            observed_ja4 = Some(val.to_string());
        } else if arg == "--ja4" {
            let val = iter.next().ok_or("--ja4 requires a value")?;
            observed_ja4 = Some(val.clone());
        } else if let Some(val) = arg.strip_prefix("--http3-perk-text=") {
            observed_http3_perk_text = Some(val.to_string());
        } else if arg == "--http3-perk-text" {
            let val = iter.next().ok_or("--http3-perk-text requires a value")?;
            observed_http3_perk_text = Some(val.clone());
        } else if let Some(val) = arg.strip_prefix("--http3-perk-hash=") {
            observed_http3_perk_hash = Some(val.to_string());
        } else if arg == "--http3-perk-hash" {
            let val = iter.next().ok_or("--http3-perk-hash requires a value")?;
            observed_http3_perk_hash = Some(val.clone());
        } else {
            urls.push(arg.clone());
        }
    }

    Ok(ProbeArgs {
        threshold,
        urls,
        observed_ja3_hash,
        observed_ja4,
        observed_http3_perk_text,
        observed_http3_perk_hash,
    })
}
