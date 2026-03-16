//! Cloudflare Browser Rendering crawl adapter
//!
//! Delegates whole-site crawling to Cloudflare's `/crawl` endpoint (open beta).
//! Useful when managed, infrastructure-free crawling is preferred over running a
//! local Chrome pool — trades stealth/anti-detection capability for zero operational
//! overhead on the scraping infrastructure.
//!
//! # Feature flag
//!
//! Gated behind `cloudflare-crawl`. Enable in `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! stygian-graph = { version = "...", features = ["cloudflare-crawl"] }
//! ```
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::cloudflare_crawl::CloudflareCrawlAdapter;
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = CloudflareCrawlAdapter::new().unwrap();
//! let input = ServiceInput {
//!     url: "https://docs.example.com".to_string(),
//!     params: json!({
//!         "account_id": "abc123",
//!         "api_token":  "my-cf-token",
//!         "output_format": "markdown",
//!         "max_depth": 3,
//!         "max_pages": 50,
//!     }),
//! };
//! // let output = adapter.execute(input).await.unwrap();
//! # });
//! ```

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::time::{interval, timeout};
use tracing::{debug, info, warn};

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Base URL for the Cloudflare Browser Rendering API.
const CF_API_BASE: &str = "https://api.cloudflare.com/client/v4/accounts";

/// Default interval between poll attempts.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Default maximum time to wait for a crawl job to complete.
const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(300);

// ─── Config ───────────────────────────────────────────────────────────────────

/// Configuration for the Cloudflare crawl adapter.
///
/// All fields except `account_id` and `api_token` are optional. They map to the
/// corresponding fields in `ServiceInput.params` and can be overridden per-request.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::cloudflare_crawl::CloudflareCrawlConfig;
/// use std::time::Duration;
///
/// let config = CloudflareCrawlConfig {
///     poll_interval: Duration::from_secs(3),
///     job_timeout:   Duration::from_secs(120),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CloudflareCrawlConfig {
    /// How often to poll for job completion (default: 2 s).
    pub poll_interval: Duration,
    /// Hard timeout for waiting on any single crawl job (default: 5 min).
    pub job_timeout: Duration,
}

impl Default for CloudflareCrawlConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            job_timeout: DEFAULT_JOB_TIMEOUT,
        }
    }
}

// ─── Crawler ──────────────────────────────────────────────────────────────────

/// Cloudflare Browser Rendering crawl adapter.
///
/// Submits a seed URL to the Cloudflare `/crawl` endpoint, polls until the job
/// completes, then aggregates all page results into a single [`ServiceOutput`].
///
/// Required [`ServiceInput::params`] fields: `account_id`, `api_token`.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::cloudflare_crawl::CloudflareCrawlAdapter;
/// use stygian_graph::ports::{ScrapingService, ServiceInput};
/// use serde_json::json;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let adapter = CloudflareCrawlAdapter::new().unwrap();
/// let input = ServiceInput {
///     url: "https://docs.example.com".to_string(),
///     params: json!({
///         "account_id": "abc123",
///         "api_token":  "my-cf-token",
///         "max_depth":  2,
///     }),
/// };
/// // let output = adapter.execute(input).await.unwrap();
/// # });
/// ```
pub struct CloudflareCrawlAdapter {
    client: Client,
    config: CloudflareCrawlConfig,
}

impl CloudflareCrawlAdapter {
    /// Create an adapter with default configuration and a shared reqwest client.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::cloudflare_crawl::CloudflareCrawlAdapter;
    /// let adapter = CloudflareCrawlAdapter::new().unwrap();
    /// ```
    pub fn new() -> Result<Self> {
        Self::with_config(CloudflareCrawlConfig::default())
    }

    /// Create an adapter with custom poll / timeout settings.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::cloudflare_crawl::{
    ///     CloudflareCrawlAdapter, CloudflareCrawlConfig,
    /// };
    /// use std::time::Duration;
    ///
    /// let adapter = CloudflareCrawlAdapter::with_config(CloudflareCrawlConfig {
    ///     poll_interval: Duration::from_secs(5),
    ///     job_timeout:   Duration::from_secs(600),
    /// }).unwrap();
    /// ```
    pub fn with_config(config: CloudflareCrawlConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ServiceError::Unavailable(format!("reqwest client init failed: {e}")))?;
        Ok(Self { client, config })
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// Extract a required string field from `params`, returning a `ServiceError`
    /// with a descriptive message if missing or not a string.
    fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
        params[key].as_str().ok_or_else(|| {
            ServiceError::Unavailable(format!("missing required param: {key}")).into()
        })
    }

    /// POST the crawl job and return the `job_id`.
    #[allow(clippy::indexing_slicing)]
    async fn submit_job(
        &self,
        account_id: &str,
        api_token: &str,
        seed_url: &str,
        params: &Value,
    ) -> Result<String> {
        let url = format!("{CF_API_BASE}/{account_id}/browser-rendering/crawl");

        let mut body = json!({ "url": seed_url });

        // Optional fields — copy from params if present.
        for key in &[
            "output_format",
            "max_depth",
            "max_pages",
            "url_pattern",
            "modified_since",
            "max_age_seconds",
            "static_mode",
        ] {
            if !params[key].is_null() {
                body[key] = params[key].clone();
            }
        }

        debug!(%seed_url, %account_id, "Submitting Cloudflare crawl job");

        let resp = self
            .client
            .post(&url)
            .bearer_auth(api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ServiceError::Unavailable(format!("CF crawl submit failed: {e}")))?;

        let status = resp.status();
        let resp_body: Value = resp
            .json()
            .await
            .map_err(|e| ServiceError::InvalidResponse(format!("CF crawl response parse: {e}")))?;

        if !status.is_success() {
            let msg = extract_cf_error(&resp_body);
            return Err(
                ServiceError::Unavailable(format!("CF crawl submit HTTP {status}: {msg}")).into(),
            );
        }

        resp_body["result"]["id"]
            .as_str()
            .ok_or_else(|| {
                ServiceError::InvalidResponse("CF crawl submit: no job id in response".to_string())
                    .into()
            })
            .map(str::to_string)
    }

    /// Poll `GET …/crawl/{job_id}` until status is `"complete"` or `"failed"`,
    /// respecting `config.job_timeout` and `config.poll_interval`.
    #[allow(clippy::indexing_slicing)]
    async fn poll_job(&self, account_id: &str, api_token: &str, job_id: &str) -> Result<Value> {
        let url = format!("{CF_API_BASE}/{account_id}/browser-rendering/crawl/{job_id}");
        let poll_interval = self.config.poll_interval;
        let job_timeout = self.config.job_timeout;

        let poll = async {
            let mut ticker = interval(poll_interval);
            loop {
                ticker.tick().await;
                debug!(%job_id, "Polling Cloudflare crawl job");

                let resp = self
                    .client
                    .get(&url)
                    .bearer_auth(api_token)
                    .send()
                    .await
                    .map_err(|e| ServiceError::Unavailable(format!("CF crawl poll failed: {e}")))?;

                let http_status = resp.status();
                let body: Value = resp
                    .json()
                    .await
                    .map_err(|e| ServiceError::InvalidResponse(format!("CF poll parse: {e}")))?;

                if !http_status.is_success() {
                    let msg = extract_cf_error(&body);
                    return Err::<Value, crate::domain::error::StygianError>(
                        ServiceError::Unavailable(format!(
                            "CF crawl poll HTTP {http_status}: {msg}"
                        ))
                        .into(),
                    );
                }

                match body["result"]["status"].as_str() {
                    Some("complete") => {
                        info!(%job_id, "Cloudflare crawl job complete");
                        return Ok(body);
                    }
                    Some("failed") => {
                        let msg = extract_cf_error(&body);
                        return Err(ServiceError::Unavailable(format!(
                            "CF crawl job failed: {msg}"
                        ))
                        .into());
                    }
                    Some(other) => {
                        debug!(%job_id, status = %other, "Crawl job in progress");
                    }
                    None => {
                        warn!(%job_id, "CF crawl poll: missing status field");
                    }
                }
            }
        };

        timeout(job_timeout, poll).await.map_err(|_| {
            StygianError::from(ServiceError::Timeout(
                u64::try_from(job_timeout.as_millis()).unwrap_or(u64::MAX),
            ))
        })?
    }

    /// Aggregate completed job results into `(data, metadata)`.
    #[allow(clippy::indexing_slicing)]
    fn collect_output(completed: &Value, job_id: &str, output_format: &str) -> (String, Value) {
        let pages: &[Value] = completed["result"]["pages"]
            .as_array()
            .map_or(&[], Vec::as_slice);

        let data = pages
            .iter()
            .filter_map(|p| p["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let metadata = json!({
            "job_id":        job_id,
            "pages_crawled": pages.len(),
            "output_format": output_format,
        });

        (data, metadata)
    }
}

#[async_trait]
impl ScrapingService for CloudflareCrawlAdapter {
    /// Submit a crawl job to Cloudflare, poll until complete, and return
    /// aggregated page content.
    ///
    /// # Params
    ///
    /// `input.params` must contain `account_id` and `api_token`. Optional
    /// fields: `output_format`, `max_depth`, `max_pages`, `url_pattern`,
    /// `modified_since`, `max_age_seconds`, `static_mode`.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Unavailable`] for API errors, and
    /// [`ServiceError::Timeout`] if the job does not complete within
    /// `config.job_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::cloudflare_crawl::CloudflareCrawlAdapter;
    /// use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// use serde_json::json;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let adapter = CloudflareCrawlAdapter::new().unwrap();
    /// let input = ServiceInput {
    ///     url: "https://docs.example.com".to_string(),
    ///     params: json!({
    ///         "account_id": "abc123",
    ///         "api_token":  "my-token",
    ///     }),
    /// };
    /// // let output = adapter.execute(input).await.unwrap();
    /// # });
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let params = &input.params;

        let account_id = Self::required_str(params, "account_id")?.to_string();
        let api_token = Self::required_str(params, "api_token")?.to_string();
        let output_format = params["output_format"]
            .as_str()
            .unwrap_or("markdown")
            .to_string();

        let job_id = self
            .submit_job(&account_id, &api_token, &input.url, params)
            .await?;

        let completed = self.poll_job(&account_id, &api_token, &job_id).await?;

        let (data, metadata) = Self::collect_output(&completed, &job_id, &output_format);

        Ok(ServiceOutput { data, metadata })
    }

    fn name(&self) -> &'static str {
        "cloudflare-crawl"
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Extract a human-readable error message from a Cloudflare API response body.
///
/// Checks `errors[0].message` first, falls back to the raw body.
///
/// # Example
///
/// ```
/// use serde_json::json;
/// use stygian_graph::adapters::cloudflare_crawl::extract_cf_error;
///
/// let body = json!({ "errors": [{ "code": 1000, "message": "Invalid token" }] });
/// assert_eq!(extract_cf_error(&body), "1000: Invalid token");
/// ```
pub fn extract_cf_error(body: &Value) -> String {
    if let Some(errors) = body["errors"].as_array()
        && let Some(first) = errors.first()
    {
        let code = first["code"].as_u64().unwrap_or(0);
        let msg = first["message"].as_str().unwrap_or("unknown");
        return format!("{code}: {msg}");
    }
    body.to_string()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── extract_cf_error ──────────────────────────────────────────────────

    #[test]
    fn extract_cf_error_formats_code_and_message() {
        let body = json!({ "errors": [{ "code": 1000, "message": "Invalid token" }] });
        assert_eq!(extract_cf_error(&body), "1000: Invalid token");
    }

    #[test]
    fn extract_cf_error_falls_back_to_raw_body() {
        let body = json!({ "success": false });
        // No 'errors' key — should return the raw JSON string.
        let result = extract_cf_error(&body);
        assert!(result.contains("success"));
    }

    #[test]
    fn extract_cf_error_handles_empty_errors_array() {
        let body = json!({ "errors": [] });
        let result = extract_cf_error(&body);
        // Falls back to raw body when errors array is empty.
        assert!(result.contains("errors"));
    }

    // ── required_str ─────────────────────────────────────────────────────

    #[test]
    fn required_str_returns_value_when_present() {
        let params = json!({ "account_id": "abc123" });
        let result = CloudflareCrawlAdapter::required_str(&params, "account_id");
        assert_eq!(result.unwrap(), "abc123");
    }

    #[test]
    fn required_str_errors_when_missing() {
        let params = json!({});
        let result = CloudflareCrawlAdapter::required_str(&params, "account_id");
        assert!(result.is_err());
    }

    #[test]
    fn required_str_errors_when_not_a_string() {
        let params = json!({ "account_id": 42 });
        let result = CloudflareCrawlAdapter::required_str(&params, "account_id");
        assert!(result.is_err());
    }

    // ── collect_output ────────────────────────────────────────────────────

    #[test]
    fn collect_output_joins_page_content() {
        let completed = json!({
            "result": {
                "status": "complete",
                "pages": [
                    { "url": "https://example.com/a", "content": "# Page A" },
                    { "url": "https://example.com/b", "content": "# Page B" },
                ]
            }
        });

        let (data, meta) = CloudflareCrawlAdapter::collect_output(&completed, "job-1", "markdown");

        assert!(data.contains("# Page A"));
        assert!(data.contains("# Page B"));
        assert_eq!(meta["job_id"], "job-1");
        assert_eq!(meta["pages_crawled"], 2);
        assert_eq!(meta["output_format"], "markdown");
    }

    #[test]
    fn collect_output_handles_no_pages() {
        let completed = json!({ "result": { "status": "complete", "pages": [] } });
        let (data, meta) = CloudflareCrawlAdapter::collect_output(&completed, "job-2", "html");
        assert_eq!(data, "");
        assert_eq!(meta["pages_crawled"], 0);
    }

    #[test]
    fn collect_output_skips_pages_without_content() {
        let completed = json!({
            "result": {
                "pages": [
                    { "url": "https://example.com/a" },        // no 'content'
                    { "url": "https://example.com/b", "content": "hello" },
                ]
            }
        });
        let (data, _) = CloudflareCrawlAdapter::collect_output(&completed, "job-3", "markdown");
        assert_eq!(data, "hello");
    }

    // ── execute — missing params ───────────────────────────────────────────

    #[tokio::test]
    async fn execute_missing_account_id_returns_error() {
        let adapter = CloudflareCrawlAdapter::new().unwrap();
        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({ "api_token": "tok" }),
        };
        assert!(adapter.execute(input).await.is_err());
    }

    #[tokio::test]
    async fn execute_missing_api_token_returns_error() {
        let adapter = CloudflareCrawlAdapter::new().unwrap();
        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({ "account_id": "acc" }),
        };
        assert!(adapter.execute(input).await.is_err());
    }

    // ── Integration tests (real Cloudflare account, skipped by default) ───

    /// End-to-end integration test.
    ///
    /// Requires `CF_ACCOUNT_ID` and `CF_API_TOKEN` to be set and a valid
    /// Cloudflare Browser Rendering subscription.
    #[ignore = "requires real Cloudflare credentials and subscription"]
    #[tokio::test]
    async fn integration_real_crawl() {
        let account_id =
            std::env::var("CF_ACCOUNT_ID").expect("CF_ACCOUNT_ID must be set for integration test");
        let api_token =
            std::env::var("CF_API_TOKEN").expect("CF_API_TOKEN must be set for integration test");

        let adapter = CloudflareCrawlAdapter::with_config(CloudflareCrawlConfig {
            poll_interval: Duration::from_secs(3),
            job_timeout: Duration::from_secs(120),
        })
        .expect("test: client init");

        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({
                "account_id":    account_id,
                "api_token":     api_token,
                "output_format": "markdown",
                "max_depth":     1,
                "max_pages":     5,
            }),
        };

        let output = adapter.execute(input).await.expect("crawl should succeed");
        assert!(!output.data.is_empty(), "expected page content");
        assert_eq!(output.metadata["output_format"], "markdown");
    }
}
