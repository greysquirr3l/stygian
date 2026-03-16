//! Live integration tests against <https://crawllab.dev>.
//!
//! crawllab.dev is a free, open-source scraper testing harness that provides
//! predictable endpoints for every HTTP status code, redirect variants,
//! content types, JS-rendered pages, and more.
//!
//! These tests exercise `RestApiAdapter` against real network I/O and are
//! gated with `#[ignore]` to keep the default `cargo test` hermetic.
//!
//! Run them explicitly:
//!
//! ```sh
//! cargo test -p stygian-graph --test crawllab -- --ignored
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use std::time::Duration;

use serde_json::json;
use stygian_graph::adapters::rest_api::{RestApiAdapter, RestApiConfig};
use stygian_graph::domain::error::{ServiceError, StygianError};
use stygian_graph::ports::{ScrapingService, ServiceInput};

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Returns an adapter with retries disabled so tests run fast and error
/// classification is deterministic on the first attempt.
fn no_retry_adapter() -> RestApiAdapter {
    RestApiAdapter::with_config(RestApiConfig {
        timeout: Duration::from_secs(15),
        max_retries: 0,
        ..Default::default()
    })
}

fn input(url: &str) -> ServiceInput {
    ServiceInput {
        url: url.to_string(),
        params: json!({}),
    }
}

// ─── HTTP status code classification ──────────────────────────────────────────

/// A plain 200 OK should return `Ok(ServiceOutput)` with a non-empty body.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn status_200_returns_ok() {
    let result = no_retry_adapter()
        .execute(input("https://crawllab.dev/status/200"))
        .await;
    assert!(result.is_ok(), "expected Ok for HTTP 200, got: {result:?}");
    let out = result.unwrap();
    assert!(
        !out.data.is_empty(),
        "200 response body should be non-empty"
    );
}

/// HTTP 404 Not Found → `ServiceError::Unavailable` containing "404".
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn status_404_returns_unavailable_error() {
    let err = no_retry_adapter()
        .execute(input("https://crawllab.dev/status/404"))
        .await
        .expect_err("expected Err for HTTP 404");

    assert!(
        matches!(err, StygianError::Service(ServiceError::Unavailable(_))),
        "expected ServiceError::Unavailable, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("404"),
        "error should mention status 404, got: {msg}"
    );
}

/// HTTP 429 Too Many Requests → `ServiceError::RateLimited`.
///
/// The adapter has dedicated 429 handling that converts the response into a
/// `RateLimited` variant instead of the generic `Unavailable`.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn status_429_returns_rate_limited_error() {
    let err = no_retry_adapter()
        .execute(input("https://crawllab.dev/status/429"))
        .await
        .expect_err("expected Err for HTTP 429");

    assert!(
        matches!(err, StygianError::Service(ServiceError::RateLimited { .. })),
        "expected ServiceError::RateLimited, got: {err:?}"
    );
}

/// HTTP 500 Internal Server Error → `ServiceError::Unavailable` containing "500".
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn status_500_returns_unavailable_error() {
    let err = no_retry_adapter()
        .execute(input("https://crawllab.dev/status/500"))
        .await
        .expect_err("expected Err for HTTP 500");

    assert!(
        matches!(err, StygianError::Service(ServiceError::Unavailable(_))),
        "expected ServiceError::Unavailable, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("500"),
        "error should mention status 500, got: {msg}"
    );
}

// ─── Redirect handling ─────────────────────────────────────────────────────────

/// A 302 temporary redirect to `/status/200` should be followed transparently
/// by reqwest and resolve to `Ok(ServiceOutput)`.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn temporary_redirect_follows_to_200() {
    let result = no_retry_adapter()
        .execute(input("https://crawllab.dev/redirect/temporary-to-200"))
        .await;
    assert!(
        result.is_ok(),
        "reqwest should follow 302 transparently; got: {result:?}"
    );
}

/// A 301 permanent redirect to `/status/200` should be followed transparently.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn permanent_redirect_follows_to_200() {
    let result = no_retry_adapter()
        .execute(input("https://crawllab.dev/redirect/permanent-to-200"))
        .await;
    assert!(
        result.is_ok(),
        "reqwest should follow 301 transparently; got: {result:?}"
    );
}

/// The cycle-a → cycle-b → cycle-a loop exhausts reqwest's redirect budget
/// and should surface as `ServiceError::Unavailable` (wrapped client error).
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn redirect_cycle_returns_error() {
    let err = no_retry_adapter()
        .execute(input("https://crawllab.dev/redirect/cycle-a"))
        .await
        .expect_err("infinite redirect cycle should produce an error");

    assert!(
        matches!(err, StygianError::Service(ServiceError::Unavailable(_))),
        "expected ServiceError::Unavailable for redirect loop, got: {err:?}"
    );
}

// ─── Content-type negotiation ─────────────────────────────────────────────────

/// `/json` returns `application/json` — adapter should parse it as structured
/// data and serialise to a non-empty string.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn json_endpoint_returns_non_empty_data() {
    let out = no_retry_adapter()
        .execute(input("https://crawllab.dev/json"))
        .await
        .expect("json endpoint should succeed");

    assert!(!out.data.is_empty(), "/json body should be non-empty");
}

/// `/text` returns `text/plain` — adapter wraps it as a JSON string value.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn text_endpoint_returns_non_empty_data() {
    let out = no_retry_adapter()
        .execute(input("https://crawllab.dev/text"))
        .await
        .expect("text endpoint should succeed");

    assert!(!out.data.is_empty(), "/text body should be non-empty");
}

// ─── Edge cases ───────────────────────────────────────────────────────────────

/// HTTP 204 No Content is a 2xx status so the adapter must not raise an error,
/// even though there is no response body.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn empty_204_response_is_ok() {
    let result = no_retry_adapter()
        .execute(input("https://crawllab.dev/empty"))
        .await;

    assert!(
        result.is_ok(),
        "204 No Content is 2xx and must not be treated as an error, got: {result:?}"
    );
}

/// `/random` returns different content on every request — the adapter should
/// still succeed because the status is 200 OK.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn random_content_endpoint_succeeds() {
    let out = no_retry_adapter()
        .execute(input("https://crawllab.dev/random"))
        .await
        .expect("/random endpoint should succeed");

    assert!(!out.data.is_empty(), "/random body should be non-empty");
}

/// Forum pages are multi-page HTML — exercises the adapter with plain HTML
/// bodies that are not JSON, confirming it wraps them as JSON strings.
#[tokio::test]
#[ignore = "requires network access to crawllab.dev"]
async fn forum_page_html_body_is_non_empty() {
    let out = no_retry_adapter()
        .execute(input("https://crawllab.dev/forum?page=1"))
        .await
        .expect("forum page 1 should succeed");

    assert!(
        !out.data.is_empty(),
        "/forum?page=1 HTML body should not be empty"
    );
    // The metadata should record page_count=1 (single-shot, no pagination params).
    assert_eq!(
        out.metadata["page_count"].as_u64(),
        Some(1),
        "single request should record page_count=1"
    );
}
