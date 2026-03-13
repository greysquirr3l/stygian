//! REST API scraping adapter with authentication and pagination support.
//!
//! Implements [`ScrapingService`] for structured REST JSON APIs. Supports:
//!
//! - HTTP methods: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`
//! - Authentication: Bearer token, HTTP Basic, API key (header or query param)
//! - Automatic pagination: offset/page, cursor, or RFC 8288 `Link` header
//! - JSON response data extraction via dot-separated path
//! - Custom request headers and query string parameters
//! - Configurable retries with exponential backoff
//!
//! All per-request options live in [`ServiceInput::params`]; see the
//! [`RestApiAdapter::execute`] docs for the full contract.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::rest_api::{RestApiAdapter, RestApiConfig};
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//! use std::time::Duration;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = RestApiAdapter::with_config(RestApiConfig {
//!     timeout:      Duration::from_secs(20),
//!     max_retries:  2,
//!     ..Default::default()
//! });
//!
//! let input = ServiceInput {
//!     url: "https://api.github.com/repos/rust-lang/rust/issues".to_string(),
//!     params: json!({
//!         "auth": { "type": "bearer", "token": "ghp_..." },
//!         "query": { "state": "open", "per_page": "30" },
//!         "pagination": { "strategy": "link_header", "max_pages": 5 },
//!         "response": { "data_path": "" }
//!     }),
//! };
//! // let output = adapter.execute(input).await.unwrap();
//! # });
//! ```

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Method, Proxy, header};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Config ───────────────────────────────────────────────────────────────────

/// Configuration for [`RestApiAdapter`].
///
/// Adapter-level defaults; per-request settings come from `ServiceInput.params`.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::rest_api::RestApiConfig;
/// use std::time::Duration;
///
/// let cfg = RestApiConfig {
///     timeout:          Duration::from_secs(20),
///     max_retries:      2,
///     retry_base_delay: Duration::from_millis(500),
///     proxy_url:        None,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct RestApiConfig {
    /// Per-request timeout (default: 30 s).
    pub timeout: Duration,
    /// Maximum retry attempts per page request on transient errors (default: 3).
    pub max_retries: u32,
    /// Base delay for exponential backoff (default: 1 s).
    pub retry_base_delay: Duration,
    /// Optional HTTP/HTTPS/SOCKS5 proxy URL.
    pub proxy_url: Option<String>,
}

impl Default for RestApiConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_base_delay: Duration::from_secs(1),
            proxy_url: None,
        }
    }
}

// ─── Internal request model ───────────────────────────────────────────────────

/// Authentication scheme, parsed from `params.auth`.
#[derive(Debug, Clone)]
enum AuthScheme {
    /// No authentication.
    None,
    /// `Authorization: Bearer <token>`
    Bearer(String),
    /// HTTP Basic authentication.
    Basic { username: String, password: String },
    /// Arbitrary header: `<header>: <key>`
    ApiKeyHeader { header: String, key: String },
    /// Append `?<param>=<key>` to the query string.
    ApiKeyQuery { param: String, key: String },
}

/// Request body variant.
#[derive(Debug, Clone)]
enum RequestBody {
    Json(Value),
    Raw(String),
}

/// How to advance to the next page.
#[derive(Debug, Clone)]
enum PaginationStrategy {
    /// Single request — no pagination.
    None,
    /// Increment a page/offset query parameter.
    Offset {
        page_param: String,
        page_size_param: Option<String>,
        page_size: Option<u64>,
        current_page: u64,
    },
    /// Follow a cursor embedded in the response JSON.
    Cursor {
        /// Query parameter name that carries the cursor on subsequent requests.
        cursor_param: String,
        /// Dot-separated path into the response JSON where the next cursor lives.
        cursor_field: String,
    },
    /// Follow RFC 8288 `Link: <URL>; rel="next"` response header.
    LinkHeader,
}

/// Fully-parsed per-request specification, derived from `ServiceInput.params`.
#[derive(Debug, Clone)]
struct RequestSpec {
    method: Method,
    extra_headers: HashMap<String, String>,
    query_params: HashMap<String, String>,
    body: Option<RequestBody>,
    auth: AuthScheme,
    accept: String,
    /// Dot-separated path into the JSON response to extract as data.
    /// `None` means use the full response body.
    data_path: Option<String>,
    /// Return paged data as a flat JSON array even when only one page was fetched.
    collect_as_array: bool,
    pagination: PaginationStrategy,
    max_pages: usize,
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// REST API scraping adapter.
///
/// Thread-safe and cheaply cloneable — the inner `reqwest::Client` uses `Arc`
/// internally. Build once, share across tasks.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::rest_api::RestApiAdapter;
///
/// let adapter = RestApiAdapter::new();
/// ```
#[derive(Clone)]
pub struct RestApiAdapter {
    client: Client,
    config: RestApiConfig,
}

impl RestApiAdapter {
    /// Create a new adapter with default configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::rest_api::RestApiAdapter;
    /// let adapter = RestApiAdapter::new();
    /// ```
    pub fn new() -> Self {
        Self::with_config(RestApiConfig::default())
    }

    /// Create an adapter with custom configuration.
    ///
    /// # Panics
    ///
    /// Panics only if TLS is unavailable on the host (extremely rare).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::rest_api::{RestApiAdapter, RestApiConfig};
    /// use std::time::Duration;
    ///
    /// let adapter = RestApiAdapter::with_config(RestApiConfig {
    ///     timeout: Duration::from_secs(10),
    ///     ..Default::default()
    /// });
    /// ```
    pub fn with_config(config: RestApiConfig) -> Self {
        let mut builder = Client::builder()
            .timeout(config.timeout)
            .gzip(true)
            .brotli(true)
            .use_rustls_tls();

        if let Some(ref proxy_url) = config.proxy_url
            && let Ok(proxy) = Proxy::all(proxy_url)
        {
            builder = builder.proxy(proxy);
        }

        // SAFETY: TLS via rustls is always available; build() can only fail if the
        // TLS backend is completely absent, which cannot happen with use_rustls_tls().
        #[allow(clippy::expect_used)]
        let client = builder.build().expect("TLS backend unavailable");

        Self { client, config }
    }

    /// Resolve a dot-separated path into a JSON [`Value`].
    ///
    /// Returns `None` if any path segment is missing.
    ///
    /// # Example
    ///
    /// ```
    /// use serde_json::json;
    /// use stygian_graph::adapters::rest_api::RestApiAdapter;
    ///
    /// let v = json!({"meta": {"next": "abc123"}});
    /// assert_eq!(
    ///     RestApiAdapter::extract_path(&v, "meta.next"),
    ///     Some(&json!("abc123"))
    /// );
    /// assert!(RestApiAdapter::extract_path(&v, "meta.gone").is_none());
    /// ```
    pub fn extract_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
        let mut current = value;
        for segment in path.split('.') {
            current = current.get(segment)?;
        }
        Some(current)
    }

    /// Parse an RFC 8288 `Link` header and return the `rel="next"` URL, if any.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::rest_api::RestApiAdapter;
    ///
    /// let link = r#"<https://api.example.com/items?page=2>; rel="next", <https://api.example.com/items?page=1>; rel="prev""#;
    /// assert_eq!(
    ///     RestApiAdapter::parse_link_next(link),
    ///     Some("https://api.example.com/items?page=2".to_owned())
    /// );
    /// ```
    pub fn parse_link_next(link_header: &str) -> Option<String> {
        for part in link_header.split(',') {
            let part = part.trim();
            let mut url: Option<String> = None;
            let mut is_next = false;
            for segment in part.split(';') {
                let segment = segment.trim();
                if segment.starts_with('<') && segment.ends_with('>') {
                    url = Some(segment[1..segment.len() - 1].to_owned());
                } else if segment.trim_start_matches("rel=").trim_matches('"') == "next" {
                    is_next = true;
                }
            }
            if is_next {
                return url;
            }
        }
        None
    }

    /// Parse `ServiceInput.params` into a `RequestSpec`.
    #[allow(clippy::indexing_slicing)]
    fn parse_spec(params: &Value) -> Result<RequestSpec> {
        let method_str = params["method"].as_str().unwrap_or("GET").to_uppercase();
        let method = match method_str.as_str() {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "PATCH" => Method::PATCH,
            "DELETE" => Method::DELETE,
            "HEAD" => Method::HEAD,
            other => {
                return Err(StygianError::from(ServiceError::Unavailable(format!(
                    "unknown HTTP method: {other}"
                ))));
            }
        };

        let extra_headers = params["headers"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();

        let query_params = params["query"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| {
                        let s = if v.is_string() {
                            v.as_str().map(ToOwned::to_owned)
                        } else {
                            Some(v.to_string())
                        };
                        s.map(|val| (k.clone(), val))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let body = if params["body"].is_null() {
            params["body_raw"]
                .as_str()
                .map(|raw| RequestBody::Raw(raw.to_owned()))
        } else {
            Some(RequestBody::Json(params["body"].clone()))
        };

        let accept = params["accept"]
            .as_str()
            .unwrap_or("application/json")
            .to_owned();

        let auth = Self::parse_auth(&params["auth"]);

        let data_path = match params["response"]["data_path"].as_str() {
            Some("") | None => None,
            Some(p) => Some(p.to_owned()),
        };
        let collect_as_array = params["response"]["collect_as_array"]
            .as_bool()
            .unwrap_or(false);

        let max_pages = params["pagination"]["max_pages"]
            .as_u64()
            .map_or(1, |n| usize::try_from(n).unwrap_or(usize::MAX));

        let pagination = Self::parse_pagination(&params["pagination"]);

        Ok(RequestSpec {
            method,
            extra_headers,
            query_params,
            body,
            auth,
            accept,
            data_path,
            collect_as_array,
            pagination,
            max_pages,
        })
    }

    /// Parse `params.auth` into an [`AuthScheme`].
    #[allow(clippy::indexing_slicing)]
    fn parse_auth(auth: &Value) -> AuthScheme {
        match auth["type"].as_str().unwrap_or("none") {
            "bearer" | "oauth2" => auth["token"]
                .as_str()
                .map_or(AuthScheme::None, |t| AuthScheme::Bearer(t.to_owned())),
            "basic" => AuthScheme::Basic {
                username: auth["username"].as_str().unwrap_or("").to_owned(),
                password: auth["password"].as_str().unwrap_or("").to_owned(),
            },
            "api_key_header" => AuthScheme::ApiKeyHeader {
                header: auth["header"].as_str().unwrap_or("X-Api-Key").to_owned(),
                key: auth["key"].as_str().unwrap_or("").to_owned(),
            },
            "api_key_query" => AuthScheme::ApiKeyQuery {
                param: auth["param"].as_str().unwrap_or("api_key").to_owned(),
                key: auth["key"].as_str().unwrap_or("").to_owned(),
            },
            _ => AuthScheme::None,
        }
    }

    /// Parse `params.pagination` into a [`PaginationStrategy`].
    #[allow(clippy::indexing_slicing)]
    fn parse_pagination(pag: &Value) -> PaginationStrategy {
        match pag["strategy"].as_str().unwrap_or("none") {
            "offset" => PaginationStrategy::Offset {
                page_param: pag["page_param"].as_str().unwrap_or("page").to_owned(),
                page_size_param: pag["page_size_param"].as_str().map(ToOwned::to_owned),
                page_size: pag["page_size"].as_u64(),
                current_page: pag["start_page"].as_u64().unwrap_or(1),
            },
            "cursor" => PaginationStrategy::Cursor {
                cursor_param: pag["cursor_param"].as_str().unwrap_or("cursor").to_owned(),
                cursor_field: pag["cursor_field"]
                    .as_str()
                    .unwrap_or("next_cursor")
                    .to_owned(),
            },
            "link_header" => PaginationStrategy::LinkHeader,
            _ => PaginationStrategy::None,
        }
    }

    /// Extract the data portion of a parsed response using `spec.data_path`.
    fn extract_data(response: &Value, spec: &RequestSpec) -> Value {
        spec.data_path
            .as_deref()
            .and_then(|path| Self::extract_path(response, path))
            .cloned()
            .unwrap_or_else(|| response.clone())
    }

    /// Execute a single HTTP request, retrying on transient failures.
    async fn send_one(
        &self,
        url: &str,
        spec: &RequestSpec,
        extra_query: &HashMap<String, String>,
    ) -> Result<(Value, Option<String>)> {
        let mut last_err: Option<StygianError> = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = self.config.retry_base_delay * 2u32.saturating_pow(attempt - 1);
                tokio::time::sleep(delay).await;
                debug!(url, attempt, "REST API retry");
            }

            match self.do_send(url, spec, extra_query).await {
                Ok(r) => return Ok(r),
                Err(e) if is_retryable(&e) && attempt < self.config.max_retries => {
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            StygianError::from(ServiceError::Unavailable("max retries exceeded".into()))
        }))
    }

    /// Perform exactly one HTTP round-trip (no retry).
    ///
    /// Returns the parsed JSON response body and the raw `Link` header value (if present).
    async fn do_send(
        &self,
        url: &str,
        spec: &RequestSpec,
        extra_query: &HashMap<String, String>,
    ) -> Result<(Value, Option<String>)> {
        let mut req = self.client.request(spec.method.clone(), url);

        // Accept header
        req = req.header(header::ACCEPT, spec.accept.as_str());

        // Auth — header-based schemes
        req = match &spec.auth {
            AuthScheme::Bearer(token) => req.bearer_auth(token),
            AuthScheme::Basic { username, password } => req.basic_auth(username, Some(password)),
            AuthScheme::ApiKeyHeader { header: hdr, key } => req.header(hdr.as_str(), key.as_str()),
            AuthScheme::ApiKeyQuery { .. } | AuthScheme::None => req,
        };

        // Custom headers
        for (k, v) in &spec.extra_headers {
            req = req.header(k.as_str(), v.as_str());
        }

        // Merge query params: static + per-page extra + API key query (if applicable)
        let mut merged: HashMap<String, String> = spec.query_params.clone();
        merged.extend(extra_query.iter().map(|(k, v)| (k.clone(), v.clone())));
        if let AuthScheme::ApiKeyQuery { param, key } = &spec.auth {
            merged.insert(param.clone(), key.clone());
        }
        if !merged.is_empty() {
            let pairs: Vec<(&String, &String)> = merged.iter().collect();
            req = req.query(&pairs);
        }

        // Body
        req = match &spec.body {
            Some(RequestBody::Json(v)) => req.json(v),
            Some(RequestBody::Raw(s)) => req.body(s.clone()),
            None => req,
        };

        let response = req
            .send()
            .await
            .map_err(|e| StygianError::from(ServiceError::Unavailable(e.to_string())))?;

        let status = response.status();

        // Capture Link header before consuming the response
        let link_header = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned);

        // 429 — log retry-after hint
        if status.as_u16() == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5);
            warn!(url, retry_after, "REST API rate-limited (429)");
            return Err(StygianError::from(ServiceError::Unavailable(format!(
                "HTTP 429 rate-limited; retry-after={retry_after}s"
            ))));
        }

        if !status.is_success() {
            let snippet: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(StygianError::from(ServiceError::Unavailable(format!(
                "HTTP {status}: {snippet}"
            ))));
        }

        let body = response
            .text()
            .await
            .map_err(|e| StygianError::from(ServiceError::Unavailable(e.to_string())))?;

        // Parse as JSON when possible; wrap plain text as a JSON string otherwise.
        let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));

        Ok((parsed, link_header))
    }
}

impl Default for RestApiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Returns `true` for transient errors that are worth retrying.
fn is_retryable(err: &StygianError) -> bool {
    let StygianError::Service(ServiceError::Unavailable(msg)) = err else {
        return false;
    };
    msg.contains("429")
        || msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("connection")
        || msg.contains("timed out")
}

// ─── ScrapingService ──────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for RestApiAdapter {
    /// Execute one or more REST API requests and return the aggregated result.
    ///
    /// # `ServiceInput.url`
    ///
    /// Base URL of the REST endpoint (including path; query string is optional).
    ///
    /// # `ServiceInput.params` contract
    ///
    /// ```json
    /// {
    ///   "method":   "GET",
    ///   "body":     { "key": "value" },
    ///   "body_raw": "raw body string",
    ///   "headers":  { "X-Custom-Header": "value" },
    ///   "query":    { "state": "open", "per_page": "30" },
    ///   "accept":   "application/json",
    ///
    ///   "auth": {
    ///     "type":     "bearer",
    ///     "token":    "...",
    ///     "username": "user",
    ///     "password": "pass",
    ///     "header":   "X-Api-Key",
    ///     "param":    "api_key",
    ///     "key":      "sk-..."
    ///   },
    ///
    ///   "response": {
    ///     "data_path":        "items",
    ///     "collect_as_array": true
    ///   },
    ///
    ///   "pagination": {
    ///     "strategy":        "link_header",
    ///     "max_pages":       10,
    ///     "page_param":      "page",
    ///     "page_size_param": "per_page",
    ///     "page_size":       100,
    ///     "start_page":      1,
    ///     "cursor_param":    "cursor",
    ///     "cursor_field":    "meta.next_cursor"
    ///   }
    /// }
    /// ```
    ///
    /// # Auth `type` values
    ///
    /// | `type` | Required fields | Description |
    /// |---|---|---|
    /// | `"bearer"` / `"oauth2"` | `token` | `Authorization: Bearer <token>` |
    /// | `"basic"` | `username`, `password` | HTTP Basic |
    /// | `"api_key_header"` | `header`, `key` | Custom header |
    /// | `"api_key_query"` | `param`, `key` | Query string |
    /// | `"none"` or absent | — | No auth |
    ///
    /// # Pagination strategies
    ///
    /// | `strategy` | Description |
    /// |---|---|
    /// | `"none"` | Single request (default) |
    /// | `"offset"` | Increment `page_param` from `start_page` |
    /// | `"cursor"` | Extract next cursor at `cursor_field` in each response; pass it as `cursor_param` |
    /// | `"link_header"` | Follow RFC 8288 `Link: <url>; rel="next"` header |
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let spec = Self::parse_spec(&input.params)?;

        let mut accumulated: Vec<Value> = Vec::new();
        let mut page_count: usize = 0;
        let mut current_url = input.url.clone();
        let mut pagination = spec.pagination.clone();
        let mut extra_query: HashMap<String, String> = HashMap::new();

        // Cursor state lives outside the loop so it persists across pages.
        let mut cursor_state: Option<String> = None;

        info!(url = %input.url, "REST API execute start");

        loop {
            if page_count >= spec.max_pages {
                debug!(%current_url, page_count, "REST API: max_pages reached");
                break;
            }

            // Build per-page query additions
            extra_query.clear();
            match &pagination {
                PaginationStrategy::Offset {
                    page_param,
                    page_size_param,
                    page_size,
                    current_page,
                } => {
                    extra_query.insert(page_param.clone(), current_page.to_string());
                    if let (Some(size_param), Some(size)) = (page_size_param, page_size) {
                        extra_query.insert(size_param.clone(), size.to_string());
                    }
                }
                PaginationStrategy::Cursor { cursor_param, .. } => {
                    if let Some(ref cursor) = cursor_state {
                        extra_query.insert(cursor_param.clone(), cursor.clone());
                    }
                }
                PaginationStrategy::None | PaginationStrategy::LinkHeader => {}
            }

            let (response, link_header) = self.send_one(&current_url, &spec, &extra_query).await?;

            let page_data = Self::extract_data(&response, &spec);

            // Accumulate — empty array responses signal end-of-pagination.
            match &page_data {
                Value::Array(items) => {
                    if items.is_empty() {
                        debug!("REST API: empty page, stopping pagination");
                        break;
                    }
                    accumulated.extend(items.iter().cloned());
                }
                other => {
                    accumulated.push(other.clone());
                }
            }
            page_count += 1;

            // Advance pagination state
            let stop = match &mut pagination {
                PaginationStrategy::None => true,
                PaginationStrategy::Offset { current_page, .. } => {
                    *current_page += 1;
                    false
                }
                PaginationStrategy::Cursor { cursor_field, .. } => {
                    Self::extract_path(&response, cursor_field.as_str())
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .is_none_or(|cursor| {
                            cursor_state = Some(cursor);
                            false
                        })
                }
                PaginationStrategy::LinkHeader => link_header
                    .as_deref()
                    .and_then(Self::parse_link_next)
                    .is_none_or(|next_url| {
                        current_url = next_url;
                        false
                    }),
            };
            if stop {
                break;
            }
        }

        // Serialise accumulated results
        let data_value = if spec.collect_as_array || accumulated.len() > 1 {
            Value::Array(accumulated)
        } else {
            accumulated.into_iter().next().unwrap_or(Value::Null)
        };

        let data_str = match &data_value {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        };

        let metadata = json!({
            "url":        input.url,
            "page_count": page_count,
        });

        info!(%input.url, page_count, "REST API execute done");

        Ok(ServiceOutput {
            data: data_str,
            metadata,
        })
    }

    fn name(&self) -> &'static str {
        "rest-api"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── parse_auth ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_auth_bearer() {
        let auth = json!({"type": "bearer", "token": "tok123"});
        match RestApiAdapter::parse_auth(&auth) {
            AuthScheme::Bearer(t) => assert_eq!(t, "tok123"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_auth_oauth2_alias() {
        let auth = json!({"type": "oauth2", "token": "oauth_tok"});
        match RestApiAdapter::parse_auth(&auth) {
            AuthScheme::Bearer(t) => assert_eq!(t, "oauth_tok"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_auth_basic() {
        let auth = json!({"type": "basic", "username": "alice", "password": "s3cr3t"});
        match RestApiAdapter::parse_auth(&auth) {
            AuthScheme::Basic { username, password } => {
                assert_eq!(username, "alice");
                assert_eq!(password, "s3cr3t");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_auth_api_key_header() {
        let auth = json!({"type": "api_key_header", "header": "X-Token", "key": "k123"});
        match RestApiAdapter::parse_auth(&auth) {
            AuthScheme::ApiKeyHeader { header, key } => {
                assert_eq!(header, "X-Token");
                assert_eq!(key, "k123");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_auth_api_key_query() {
        let auth = json!({"type": "api_key_query", "param": "api_key", "key": "qk"});
        match RestApiAdapter::parse_auth(&auth) {
            AuthScheme::ApiKeyQuery { param, key } => {
                assert_eq!(param, "api_key");
                assert_eq!(key, "qk");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_auth_none_default() {
        let auth = json!(null);
        assert!(matches!(
            RestApiAdapter::parse_auth(&auth),
            AuthScheme::None
        ));
    }

    // ── extract_path ───────────────────────────────────────────────────────────

    #[test]
    fn extract_path_top_level() {
        let v = json!({"items": [1, 2, 3]});
        assert_eq!(
            RestApiAdapter::extract_path(&v, "items"),
            Some(&json!([1, 2, 3]))
        );
    }

    #[test]
    fn extract_path_nested() {
        let v = json!({"meta": {"next_cursor": "abc"}});
        assert_eq!(
            RestApiAdapter::extract_path(&v, "meta.next_cursor"),
            Some(&json!("abc"))
        );
    }

    #[test]
    fn extract_path_missing() {
        let v = json!({"a": {"b": 1}});
        assert!(RestApiAdapter::extract_path(&v, "a.c").is_none());
    }

    // ── parse_link_next ────────────────────────────────────────────────────────

    #[test]
    fn parse_link_next_present() {
        let h = r#"<https://api.example.com/items?page=2>; rel="next", <https://api.example.com/items?page=1>; rel="prev""#;
        assert_eq!(
            RestApiAdapter::parse_link_next(h),
            Some("https://api.example.com/items?page=2".to_owned())
        );
    }

    #[test]
    fn parse_link_next_absent() {
        let h = r#"<https://api.example.com/items?page=1>; rel="prev""#;
        assert!(RestApiAdapter::parse_link_next(h).is_none());
    }

    #[test]
    fn parse_link_next_single() {
        let h = r#"<https://api.example.com/items?page=3>; rel="next""#;
        assert_eq!(
            RestApiAdapter::parse_link_next(h),
            Some("https://api.example.com/items?page=3".to_owned())
        );
    }

    // ── parse_spec ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_spec_defaults() {
        let spec = RestApiAdapter::parse_spec(&json!({})).unwrap();
        assert_eq!(spec.method, Method::GET);
        assert_eq!(spec.accept, "application/json");
        assert_eq!(spec.max_pages, 1);
        assert!(spec.data_path.is_none());
        assert!(!spec.collect_as_array);
        assert!(matches!(spec.pagination, PaginationStrategy::None));
    }

    #[test]
    fn parse_spec_post_with_body_and_headers() {
        let params = json!({
            "method":  "POST",
            "body":    { "key": "value" },
            "headers": { "X-Foo": "bar" },
            "query":   { "limit": "10" }
        });
        let spec = RestApiAdapter::parse_spec(&params).unwrap();
        assert_eq!(spec.method, Method::POST);
        assert_eq!(spec.extra_headers.get("X-Foo"), Some(&"bar".to_string()));
        assert_eq!(spec.query_params.get("limit"), Some(&"10".to_string()));
        assert!(matches!(spec.body, Some(RequestBody::Json(_))));
    }

    #[test]
    fn parse_spec_unknown_method_returns_error() {
        let result = RestApiAdapter::parse_spec(&json!({"method": "BREW"}));
        assert!(result.is_err());
    }

    #[test]
    fn parse_spec_cursor_pagination() {
        let params = json!({
            "pagination": {
                "strategy":     "cursor",
                "cursor_param": "after",
                "cursor_field": "page_info.end_cursor",
                "max_pages":    10
            }
        });
        let spec = RestApiAdapter::parse_spec(&params).unwrap();
        assert_eq!(spec.max_pages, 10);
        match spec.pagination {
            PaginationStrategy::Cursor {
                cursor_param,
                cursor_field,
            } => {
                assert_eq!(cursor_param, "after");
                assert_eq!(cursor_field, "page_info.end_cursor");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_spec_offset_pagination() {
        let params = json!({
            "pagination": {
                "strategy":        "offset",
                "page_param":      "page",
                "page_size_param": "per_page",
                "page_size":       50,
                "start_page":      1,
                "max_pages":       3
            }
        });
        let spec = RestApiAdapter::parse_spec(&params).unwrap();
        assert_eq!(spec.max_pages, 3);
        match spec.pagination {
            PaginationStrategy::Offset {
                page_size,
                current_page,
                page_param,
                ..
            } => {
                assert_eq!(page_size, Some(50));
                assert_eq!(current_page, 1);
                assert_eq!(page_param, "page");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_spec_link_header_pagination() {
        let params = json!({
            "pagination": { "strategy": "link_header", "max_pages": 5 }
        });
        let spec = RestApiAdapter::parse_spec(&params).unwrap();
        assert_eq!(spec.max_pages, 5);
        assert!(matches!(spec.pagination, PaginationStrategy::LinkHeader));
    }

    #[test]
    fn parse_spec_data_path_and_collect_as_array() {
        let params = json!({
            "response": { "data_path": "data.items", "collect_as_array": true }
        });
        let spec = RestApiAdapter::parse_spec(&params).unwrap();
        assert_eq!(spec.data_path, Some("data.items".to_owned()));
        assert!(spec.collect_as_array);
    }

    #[test]
    fn parse_spec_empty_data_path_is_none() {
        let params = json!({ "response": { "data_path": "" } });
        let spec = RestApiAdapter::parse_spec(&params).unwrap();
        assert!(spec.data_path.is_none());
    }

    // ── adapter_name ───────────────────────────────────────────────────────────

    #[test]
    fn adapter_name() {
        assert_eq!(RestApiAdapter::new().name(), "rest-api");
    }

    // ── is_retryable ────────────────────────────────────────────────────────────

    #[test]
    fn is_retryable_429() {
        let e = StygianError::from(ServiceError::Unavailable(
            "HTTP 429 rate-limited".to_string(),
        ));
        assert!(is_retryable(&e));
    }

    #[test]
    fn is_retryable_503() {
        let e = StygianError::from(ServiceError::Unavailable(
            "HTTP 503 Service Unavailable".to_string(),
        ));
        assert!(is_retryable(&e));
    }

    #[test]
    fn is_retryable_404_not_retryable() {
        let e = StygianError::from(ServiceError::Unavailable("HTTP 404 Not Found".to_string()));
        assert!(!is_retryable(&e));
    }

    // ── integration ────────────────────────────────────────────────────────────

    /// Real HTTP integration test — requires `REST_API_TEST_URL` env var.
    ///
    /// Run with: `REST_API_TEST_URL=https://httpbin.org/get cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "requires live REST API endpoint; set REST_API_TEST_URL env var"]
    async fn integration_get_httpbin() {
        let url = std::env::var("REST_API_TEST_URL")
            .unwrap_or_else(|_| "https://httpbin.org/get".to_string());

        let adapter = RestApiAdapter::new();
        let input = ServiceInput {
            url,
            params: json!({}),
        };
        let output = adapter.execute(input).await.unwrap();
        assert!(!output.data.is_empty());
        assert_eq!(output.metadata["page_count"], 1);
    }
}
