#[cfg(feature = "caching")]
use std::fmt::Write;
#[cfg(feature = "caching")]
use std::num::NonZeroUsize;
#[cfg(feature = "caching")]
use std::time::{Duration, Instant};

#[cfg(feature = "caching")]
use stygian_charon::{
    InvestigationReportCache, MemoryInvestigationCache, TargetClass,
    investigate_har_cached_with_target_class,
};

#[cfg(feature = "caching")]
fn make_har(total_requests: u32, blocked_requests: u32) -> String {
    let mut entries = String::new();
    for index in 0..total_requests {
        let status = if index < blocked_requests { 403 } else { 200 };
        let status_text = if status == 403 { "Forbidden" } else { "OK" };
        let _ = write!(
            entries,
            r#"{{
                "pageref": "page1",
                "startedDateTime": "2026-01-01T00:00:{:02}Z",
                "time": 0.1,
                "request": {{
                    "method": "GET",
                    "url": "https://example.com/api/resource{}",
                    "headers": [],
                    "queryString": [],
                    "cookies": [],
                    "headersSize": 0,
                    "bodySize": 0
                }},
                "response": {{
                    "status": {},
                    "statusText": "{}",
                    "headers": [],
                    "cookies": [],
                    "content": {{"size": 128, "mimeType": "application/json"}},
                    "redirectURL": "",
                    "headersSize": 0,
                    "bodySize": 128,
                    "time": 0.05
                }},
                "cache": {{}},
                "timings": {{"blocked": -1, "dns": -1, "connect": -1, "send": 0, "wait": 50, "receive": 50}}
            }}"#,
            index % 60,
            index,
            status,
            status_text,
        );

        if index + 1 < total_requests {
            entries.push(',');
        }
    }

    format!(
        r#"{{
            "log": {{
                "version": "1.2",
                "creator": {{"name": "cache-benchmark", "version": "1.0"}},
                "pages": [{{
                    "id": "page1",
                    "title": "https://example.com",
                    "startedDateTime": "2026-01-01T00:00:00Z",
                    "pageTimings": {{"onLoad": 0}}
                }}],
                "entries": [{entries}]
            }}
        }}"#,
    )
}

#[cfg(feature = "caching")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let warmup_runs: u32 = 5;
    let uncached_runs: u32 = 100;
    let cached_runs: u32 = 100;

    let har = make_har(1000, 120);
    let cache = MemoryInvestigationCache::new(
        NonZeroUsize::new(128).ok_or("cache capacity must be non-zero")?,
        Duration::from_mins(5),
    );

    for _ in 0..warmup_runs {
        let _ = investigate_har_cached_with_target_class(&har, TargetClass::ContentSite, &cache)?;
        cache.clear();
    }

    let uncached_start = Instant::now();
    for _ in 0..uncached_runs {
        cache.clear();
        let _ = investigate_har_cached_with_target_class(&har, TargetClass::ContentSite, &cache)?;
    }
    let uncached_elapsed = uncached_start.elapsed();

    cache.clear();
    let cached_start = Instant::now();
    for _ in 0..cached_runs {
        let _ = investigate_har_cached_with_target_class(&har, TargetClass::ContentSite, &cache)?;
    }
    let cached_elapsed = cached_start.elapsed();

    let uncached_avg_us = uncached_elapsed.as_micros() / u128::from(uncached_runs);
    let cached_avg_us = cached_elapsed.as_micros() / u128::from(cached_runs);
    let improvement_basis_points = if uncached_avg_us == 0 {
        0_u128
    } else {
        uncached_avg_us
            .saturating_sub(cached_avg_us)
            .saturating_mul(10_000)
            .checked_div(uncached_avg_us)
            .unwrap_or_default()
    };
    let improvement_whole = improvement_basis_points / 100;
    let improvement_fractional = improvement_basis_points % 100;

    println!("cache benchmark");
    println!("warmup_runs={warmup_runs}");
    println!(
        "uncached_runs={uncached_runs} uncached_total_ms={}",
        uncached_elapsed.as_millis()
    );
    println!(
        "cached_runs={cached_runs} cached_total_ms={}",
        cached_elapsed.as_millis()
    );
    println!("uncached_avg_us={uncached_avg_us}");
    println!("cached_avg_us={cached_avg_us}");
    println!("improvement_pct={improvement_whole}.{improvement_fractional:02}");

    Ok(())
}

#[cfg(not(feature = "caching"))]
fn main() {
    eprintln!(
        "cache_benchmark requires the 'caching' feature. Run with: cargo run -p stygian-charon --features caching --example cache_benchmark"
    );
}
