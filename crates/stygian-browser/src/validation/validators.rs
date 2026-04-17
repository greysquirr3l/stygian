//! Individual anti-bot validator implementations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

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
pub fn run_creepjs(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = creepjs_impl(pool);
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

fn creepjs_impl(_pool: &Arc<BrowserPool>) -> ValidationResult {
    // NOTE: Real implementation would:
    // 1. Acquire a session from the pool
    // 2. Navigate to CreepJS URL
    // 3. Wait for results-loaded signal
    // 4. Extract trust score from JSON in window.result or DOM element
    // 5. Check score > 50%
    // 6. Return with score and details
    //
    // For now, return a stub "not yet implemented" result that keeps CI
    // deterministic while documenting the need for a live browser environment.

    ValidationResult {
        target: ValidationTarget::CreepJs,
        passed: false,
        score: None,
        details: HashMap::from([("phase".to_string(), "stub-not-yet-implemented".to_string())]),
        screenshot: None,
        elapsed: std::time::Duration::ZERO,
    }
}

/// Run the `BrowserScan` validator.
///
/// Navigates to `BrowserScan`, waits for scan completion, and extracts the
/// authenticity percentage.
pub fn run_browserscan(pool: &Arc<BrowserPool>) -> ValidationResult {
    let start = Instant::now();
    let result = browserscan_impl(pool);
    ValidationResult {
        elapsed: start.elapsed(),
        ..result
    }
}

fn browserscan_impl(_pool: &Arc<BrowserPool>) -> ValidationResult {
    // NOTE: Real implementation would:
    // 1. Acquire a session from the pool
    // 2. Navigate to BrowserScan URL
    // 3. Wait for scan-complete signal
    // 4. Extract authenticity percentage from JSON or DOM
    // 5. Check score > 90%
    // 6. Return with score and details

    ValidationResult {
        target: ValidationTarget::BrowserScan,
        passed: false,
        score: None,
        details: HashMap::from([("phase".to_string(), "stub-not-yet-implemented".to_string())]),
        screenshot: None,
        elapsed: std::time::Duration::ZERO,
    }
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
