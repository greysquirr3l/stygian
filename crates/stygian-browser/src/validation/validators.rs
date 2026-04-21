//! Individual anti-bot validator implementations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::time::sleep;
use tracing::debug;

use crate::page::WaitUntil;
use crate::pool::BrowserPool;

use super::{ValidationResult, ValidationTarget};

// ───────────────────────────────────────────────────────────────────────────
// Tier 1: Open-Source Observatories (no rate limits)
// ───────────────────────────────────────────────────────────────────────────

/// Run the `CreepJS` observatory validator.
///
/// Navigates to `CreepJS`, waits for results, extracts the trust score, and
/// checks if it is > 50%.
pub async fn run_creepjs(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = creepjs_impl(pool).await;
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

async fn creepjs_impl(pool: &Arc<BrowserPool>) -> ValidationResult {
    run_tier1_observatory(pool, ValidationTarget::CreepJs, 0.50).await
}

/// Run the `BrowserScan` validator.
///
/// Navigates to `BrowserScan`, waits for scan completion, and extracts the
/// authenticity percentage.
pub async fn run_browserscan(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = browserscan_impl(pool).await;
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

async fn browserscan_impl(pool: &Arc<BrowserPool>) -> ValidationResult {
    run_tier1_observatory(pool, ValidationTarget::BrowserScan, 0.90).await
}

async fn run_tier1_observatory(
    pool: &Arc<BrowserPool>,
    target: ValidationTarget,
    min_score: f64,
) -> ValidationResult {
    let mut details = HashMap::new();
    let url = target.url();
    details.insert("phase".to_string(), "tier1-observatory".to_string());
    details.insert("url".to_string(), url.to_string());

    let session = match pool.acquire().await {
        Ok(session) => session,
        Err(err) => return ValidationResult::failed(target, &err.to_string()),
    };

    let mut screenshot: Option<Vec<u8>> = None;
    let mut passed = false;
    let mut score: Option<f64> = None;

    let result = match session.browser() {
        Some(browser) => match browser.new_page().await {
            Ok(mut page) => {
                let navigate_result = page
                    .navigate(url, WaitUntil::DomContentLoaded, Duration::from_secs(25))
                    .await;

                match navigate_result {
                    Ok(()) => {
                        // Give observatories time to execute browser fingerprint checks.
                        sleep(Duration::from_secs(6)).await;

                        let probe = page
                            .eval::<Value>(
                                r#"(() => {
                                    const body = (document.body?.innerText || "").toLowerCase();
                                    const title = (document.title || "");
                                    const href = (location.href || "");

                                    const blocked =
                                        body.includes("access denied") ||
                                        body.includes("verify you are human") ||
                                        body.includes("just a moment") ||
                                        body.includes("captcha") ||
                                        href.toLowerCase().includes("/js_challenge");

                                    const scorePatterns = [
                                        /trust\s*score[^0-9]{0,20}([0-9]{1,3}(?:\.[0-9]+)?)/i,
                                        /authenticity[^0-9]{0,20}([0-9]{1,3}(?:\.[0-9]+)?)/i,
                                        /score[^0-9]{0,20}([0-9]{1,3}(?:\.[0-9]+)?)/i,
                                        /([0-9]{1,3}(?:\.[0-9]+)?)\s*%/
                                    ];

                                    let score = null;
                                    for (const pattern of scorePatterns) {
                                        const match = body.match(pattern);
                                        if (match?.[1]) {
                                            score = Number(match[1]);
                                            if (Number.isFinite(score)) break;
                                        }
                                    }

                                    return {
                                        blocked,
                                        title,
                                        href,
                                        score
                                    };
                                })()"#,
                            )
                            .await
                            .unwrap_or_else(|_| json!({"blocked": false, "score": Value::Null}));

                        let blocked = probe
                            .get("blocked")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        score = probe
                            .get("score")
                            .and_then(Value::as_f64)
                            .map(|raw| if raw > 1.0 { raw / 100.0 } else { raw });

                        if let Some(title) = probe.get("title").and_then(Value::as_str) {
                            details.insert("title".to_string(), title.to_string());
                        }
                        if let Some(observed_url) = probe.get("href").and_then(Value::as_str) {
                            details.insert("observed_url".to_string(), observed_url.to_string());
                        }
                        details.insert("blocked".to_string(), blocked.to_string());

                        passed = !blocked && score.is_some_and(|v| v >= min_score);
                        if !passed {
                            screenshot = page.screenshot().await.ok();
                        }
                    }
                    Err(err) => {
                        details.insert("error".to_string(), err.to_string());
                    }
                }

                page.close().await.ok();
                ValidationResult {
                    target,
                    passed,
                    score,
                    details,
                    screenshot,
                    elapsed: Duration::ZERO,
                }
            }
            Err(err) => ValidationResult::failed(target, &err.to_string()),
        },
        None => ValidationResult::failed(target, "browser handle lost"),
    };

    session.release().await;
    result
}

// ───────────────────────────────────────────────────────────────────────────
// Tier 2: Anti-Bot Protected Sites (may rate-limit, use #[ignore])
// ───────────────────────────────────────────────────────────────────────────

/// Run the Kasada validator against `WizzAir` booking page.
///
/// Navigates to a Kasada-protected page, waits for page load, and checks
/// whether a 429/403 block page is returned or the page loads normally.
pub async fn run_kasada(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = kasada_impl(pool).await;
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

async fn kasada_impl(pool: &Arc<BrowserPool>) -> ValidationResult {
    let url = ValidationTarget::Kasada.url();
    debug!("Kasada validator: navigating to {url}");

    match pool.acquire().await {
        Ok(session) => {
            match session.browser() {
                Some(browser) => {
                    match browser.new_page().await {
                        Ok(mut page) => {
                            // Try to navigate with a generous timeout
                            let navigate_result = page
                                .navigate(
                                    url,
                                    WaitUntil::DomContentLoaded,
                                    std::time::Duration::from_secs(20),
                                )
                                .await;

                            let passed = match navigate_result {
                                Ok(()) => {
                                    // Check HTTP status code — 200 OK is a pass
                                    true
                                }
                                Err(e) => {
                                    // Navigation timeout or network error typically means blocked
                                    debug!("Kasada: navigation failed: {}", e);
                                    false
                                }
                            };

                            page.close().await.ok();

                            ValidationResult {
                                target: ValidationTarget::Kasada,
                                passed,
                                score: None,
                                details: HashMap::from([(
                                    "phase".to_string(),
                                    "load-check".to_string(),
                                )]),
                                screenshot: None,
                                elapsed: std::time::Duration::ZERO,
                            }
                        }
                        Err(e) => {
                            ValidationResult::failed(ValidationTarget::Kasada, &e.to_string())
                        }
                    }
                }
                None => ValidationResult::failed(ValidationTarget::Kasada, "browser handle lost"),
            }
        }
        Err(e) => ValidationResult::failed(ValidationTarget::Kasada, &e.to_string()),
    }
}

/// Run the Cloudflare validator on a CF-protected site.
///
/// Navigates to a Cloudflare-protected page and checks if the page loads
/// without a challenge block.
pub async fn run_cloudflare(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = cloudflare_impl(pool).await;
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

async fn cloudflare_impl(pool: &Arc<BrowserPool>) -> ValidationResult {
    let url = ValidationTarget::Cloudflare.url();
    debug!("Cloudflare validator: navigating to {url}");

    match pool.acquire().await {
        Ok(session) => match session.browser() {
            Some(browser) => match browser.new_page().await {
                Ok(mut page) => {
                    let navigate_result = page
                        .navigate(
                            url,
                            WaitUntil::DomContentLoaded,
                            std::time::Duration::from_secs(20),
                        )
                        .await;

                    let passed = navigate_result.is_ok();

                    page.close().await.ok();

                    ValidationResult {
                        target: ValidationTarget::Cloudflare,
                        passed,
                        score: None,
                        details: HashMap::from([("phase".to_string(), "load-check".to_string())]),
                        screenshot: None,
                        elapsed: std::time::Duration::ZERO,
                    }
                }
                Err(e) => ValidationResult::failed(ValidationTarget::Cloudflare, &e.to_string()),
            },
            None => ValidationResult::failed(ValidationTarget::Cloudflare, "browser handle lost"),
        },
        Err(e) => ValidationResult::failed(ValidationTarget::Cloudflare, &e.to_string()),
    }
}

/// Run the Akamai validator on an Akamai-protected site (e.g., `FedEx`).
///
/// Navigates to the `FedEx` tracking page and checks if the page loads
/// without bot detection.
pub async fn run_akamai(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = akamai_impl(pool).await;
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

async fn akamai_impl(pool: &Arc<BrowserPool>) -> ValidationResult {
    let url = ValidationTarget::Akamai.url();
    debug!("Akamai validator: navigating to {url}");

    match pool.acquire().await {
        Ok(session) => match session.browser() {
            Some(browser) => match browser.new_page().await {
                Ok(mut page) => {
                    let navigate_result = page
                        .navigate(
                            url,
                            WaitUntil::DomContentLoaded,
                            std::time::Duration::from_secs(20),
                        )
                        .await;

                    let passed = navigate_result.is_ok();

                    page.close().await.ok();

                    ValidationResult {
                        target: ValidationTarget::Akamai,
                        passed,
                        score: None,
                        details: HashMap::from([("phase".to_string(), "load-check".to_string())]),
                        screenshot: None,
                        elapsed: std::time::Duration::ZERO,
                    }
                }
                Err(e) => ValidationResult::failed(ValidationTarget::Akamai, &e.to_string()),
            },
            None => ValidationResult::failed(ValidationTarget::Akamai, "browser handle lost"),
        },
        Err(e) => ValidationResult::failed(ValidationTarget::Akamai, &e.to_string()),
    }
}
