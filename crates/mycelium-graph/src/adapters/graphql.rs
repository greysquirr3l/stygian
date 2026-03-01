//! GraphQL API adapter — a generic [`ScrapingService`] for any spec-compliant
//! GraphQL endpoint.
//!
//! Handles the full request/response lifecycle: query execution, variable
//! injection, GraphQL error-envelope parsing, Jobber-style cost/throttle
//! metadata, cursor-based pagination, and pluggable auth strategies.
//!
//! Target-specific knowledge (endpoint URL, version headers, default auth) is
//! supplied by a [`GraphQlTargetPlugin`](crate::ports::graphql_plugin::GraphQlTargetPlugin)
//! resolved from an optional [`GraphQlPluginRegistry`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::application::graphql_plugin_registry::GraphQlPluginRegistry;
use crate::application::pipeline_parser::expand_template;
use crate::domain::error::{MyceliumError, Result, ServiceError};
use crate::ports::{GraphQlAuth, GraphQlAuthKind, ScrapingService, ServiceInput, ServiceOutput};

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for [`GraphQlService`].
///
/// # Example
///
/// ```rust
/// use mycelium_graph::adapters::graphql::GraphQlConfig;
///
/// let config = GraphQlConfig {
///     timeout_secs: 30,
///     max_pages: 1000,
///     user_agent: "mycelium-graph/1.0".to_string(),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct GraphQlConfig {
    /// Request timeout in seconds (default: 30)
    pub timeout_secs: u64,
    /// Maximum number of pages for cursor-paginated queries (default: 1000)
    pub max_pages: usize,
    /// User-Agent header sent with every request
    pub user_agent: String,
}

impl Default for GraphQlConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            max_pages: 1000,
            user_agent: "mycelium-graph/1.0".to_string(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Adapter
// ─────────────────────────────────────────────────────────────────────────────

/// `ScrapingService` adapter for GraphQL APIs.
///
/// Implement any spec-compliant GraphQL endpoint by constructing a
/// [`GraphQlService`] with a config and an optional plugin registry. Target
/// specifics (endpoint, version headers, auth) are supplied either via
/// `ServiceInput.params` directly or through a registered
/// [`GraphQlTargetPlugin`](crate::ports::graphql_plugin::GraphQlTargetPlugin).
///
/// # Example
///
/// ```no_run
/// use mycelium_graph::adapters::graphql::{GraphQlService, GraphQlConfig};
/// use mycelium_graph::ports::{ScrapingService, ServiceInput};
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let service = GraphQlService::new(GraphQlConfig::default(), None);
///     let input = ServiceInput {
///         url: "https://countries.trevorblades.com/".to_string(),
///         params: json!({
///             "query": "{ countries { code name } }"
///         }),
///     };
///     let output = service.execute(input).await?;
///     println!("{}", output.data);
///     Ok(())
/// }
/// ```
pub struct GraphQlService {
    client: reqwest::Client,
    config: GraphQlConfig,
    plugins: Option<Arc<GraphQlPluginRegistry>>,
}

impl GraphQlService {
    /// Create a new `GraphQlService`.
    ///
    /// `plugins` may be `None` for raw-params mode (no plugin resolution).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::adapters::graphql::{GraphQlService, GraphQlConfig};
    /// use mycelium_graph::ports::ScrapingService;
    ///
    /// let service = GraphQlService::new(GraphQlConfig::default(), None);
    /// assert_eq!(service.name(), "graphql");
    /// ```
    pub fn new(config: GraphQlConfig, plugins: Option<Arc<GraphQlPluginRegistry>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .unwrap_or_default();
        Self {
            client,
            config,
            plugins,
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Apply auth to the request builder.
    fn apply_auth(builder: reqwest::RequestBuilder, auth: &GraphQlAuth) -> reqwest::RequestBuilder {
        let token = expand_template(&auth.token);
        match auth.kind {
            GraphQlAuthKind::Bearer => builder.header("Authorization", format!("Bearer {token}")),
            GraphQlAuthKind::ApiKey => builder.header("X-Api-Key", token),
            GraphQlAuthKind::Header => {
                let name = auth.header_name.as_deref().unwrap_or("X-Api-Key");
                builder.header(name, token)
            }
            GraphQlAuthKind::None => builder,
        }
    }

    /// Parse `GraphQlAuth` from a JSON object like `{"kind":"bearer","token":"..."}`.
    fn parse_auth(val: &Value) -> Option<GraphQlAuth> {
        let kind_str = val["kind"].as_str().unwrap_or("none");
        let kind = match kind_str {
            "bearer" => GraphQlAuthKind::Bearer,
            "api_key" => GraphQlAuthKind::ApiKey,
            "header" => GraphQlAuthKind::Header,
            _ => GraphQlAuthKind::None,
        };
        if kind == GraphQlAuthKind::None {
            return None;
        }
        let token = val["token"].as_str()?.to_string();
        let header_name = val["header_name"].as_str().map(str::to_string);
        Some(GraphQlAuth {
            kind,
            token,
            header_name,
        })
    }

    /// Check whether the response body indicates throttling.
    ///
    /// Returns `Some(retry_after_ms)` on throttle detection via any of:
    /// 1. `extensions.cost.throttleStatus == "THROTTLED"`
    /// 2. Any error entry with `extensions.code == "THROTTLED"`
    /// 3. Any error message containing "throttled" (case-insensitive)
    #[allow(clippy::indexing_slicing)]
    fn detect_throttle(body: &Value) -> Option<u64> {
        // 1. extensions.cost.throttleStatus
        if body["extensions"]["cost"]["throttleStatus"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("THROTTLED"))
        {
            return Some(Self::throttle_backoff(body));
        }

        // 2 & 3. errors array
        if let Some(errors) = body["errors"].as_array() {
            for err in errors {
                if err["extensions"]["code"]
                    .as_str()
                    .is_some_and(|c| c.eq_ignore_ascii_case("THROTTLED"))
                {
                    return Some(Self::throttle_backoff(body));
                }
                if err["message"]
                    .as_str()
                    .is_some_and(|m| m.to_ascii_lowercase().contains("throttled"))
                {
                    return Some(Self::throttle_backoff(body));
                }
            }
        }

        None
    }

    /// Calculate retry back-off from `extensions.cost`.
    ///
    /// ```text
    /// deficit = maximumAvailable − currentlyAvailable
    /// ms      = (deficit / restoreRate * 1000).clamp(500, 2000)
    /// ```
    #[allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn throttle_backoff(body: &Value) -> u64 {
        let cost = &body["extensions"]["cost"];
        let max_avail = cost["maximumAvailable"].as_f64().unwrap_or(10_000.0);
        let cur_avail = cost["currentlyAvailable"].as_f64().unwrap_or(0.0);
        let restore_rate = cost["restoreRate"].as_f64().unwrap_or(500.0);
        let deficit = (max_avail - cur_avail).max(0.0);
        let ms = if restore_rate > 0.0 {
            (deficit / restore_rate * 1000.0) as u64
        } else {
            2_000
        };
        ms.clamp(500, 2_000)
    }

    /// Extract the `extensions.cost` object into a metadata-compatible [`Value`].
    #[allow(clippy::indexing_slicing)]
    fn extract_cost_metadata(body: &Value) -> Option<Value> {
        let cost = &body["extensions"]["cost"];
        if cost.is_null() || cost.is_object() && cost.as_object()?.is_empty() {
            return None;
        }
        Some(cost.clone())
    }

    /// Navigate a dot-separated JSON path like `"data.clients.pageInfo"`.
    #[allow(clippy::indexing_slicing)]
    fn json_path<'v>(root: &'v Value, path: &str) -> &'v Value {
        let mut cur = root;
        for key in path.split('.') {
            cur = &cur[key];
        }
        cur
    }

    /// Execute one GraphQL POST and return the parsed JSON body or an error.
    #[allow(clippy::indexing_slicing)]
    async fn post_query(
        &self,
        url: &str,
        query: &str,
        variables: &Value,
        operation_name: Option<&str>,
        auth: Option<&GraphQlAuth>,
        extra_headers: &HashMap<String, String>,
    ) -> Result<Value> {
        let mut body = json!({ "query": query, "variables": variables });
        if let Some(op) = operation_name {
            body["operationName"] = json!(op);
        }

        let mut builder = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");

        for (k, v) in extra_headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        if let Some(a) = auth {
            builder = Self::apply_auth(builder, a);
        }

        let resp = builder
            .json(&body)
            .send()
            .await
            .map_err(|e| MyceliumError::Service(ServiceError::Unavailable(e.to_string())))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| MyceliumError::Service(ServiceError::Unavailable(e.to_string())))?;

        if status.as_u16() >= 400 {
            return Err(MyceliumError::Service(ServiceError::Unavailable(format!(
                "HTTP {status}: {text}"
            ))));
        }

        serde_json::from_str::<Value>(&text).map_err(|e| {
            MyceliumError::Service(ServiceError::InvalidResponse(format!("invalid JSON: {e}")))
        })
    }

    /// Validate a parsed GraphQL body (errors array, missing `data` key, throttle).
    #[allow(clippy::indexing_slicing)]
    fn validate_body(body: &Value) -> Result<()> {
        // Throttle check takes priority so callers can retry with backoff.
        if let Some(retry_after_ms) = Self::detect_throttle(body) {
            return Err(MyceliumError::Service(ServiceError::RateLimited {
                retry_after_ms,
            }));
        }

        if let Some(errors) = body["errors"].as_array()
            && !errors.is_empty()
        {
            let msg = errors[0]["message"]
                .as_str()
                .unwrap_or("unknown GraphQL error")
                .to_string();
            return Err(MyceliumError::Service(ServiceError::InvalidResponse(msg)));
        }

        // `data` key is missing — explicitly null with no errors is allowed (partial response)
        if body.get("data").is_none() {
            return Err(MyceliumError::Service(ServiceError::InvalidResponse(
                "missing 'data' key in GraphQL response".to_string(),
            )));
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScrapingService impl
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for GraphQlService {
    fn name(&self) -> &'static str {
        "graphql"
    }

    /// Execute a GraphQL query.
    ///
    /// Reads `ServiceInput.params` for:
    /// - `query` (required) — the GraphQL query string
    /// - `variables` — optional JSON object
    /// - `operation_name` — optional string
    /// - `auth` — optional `{"kind": "bearer"|"api_key"|"header"|"none", "token": "..."}`
    /// - `headers` — optional extra headers object
    /// - `plugin` — optional plugin name to resolve from the registry
    /// - `pagination` — optional `{"strategy": "cursor", "page_info_path": "...", "edges_path": "...", "page_size": 50}`
    ///
    /// # Errors
    ///
    /// Returns `Err` for HTTP ≥ 400, invalid JSON, GraphQL `errors[]`, missing
    /// `data` key, throttle detection, or pagination runaway.
    #[allow(clippy::too_many_lines, clippy::indexing_slicing)]
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let params = &input.params;

        // ── 1. Resolve plugin (if any) ────────────────────────────────────
        let plugin_name = params["plugin"].as_str();
        let plugin = if let (Some(name), Some(registry)) = (plugin_name, &self.plugins) {
            Some(registry.get(name)?)
        } else {
            None
        };

        // ── 2. Resolve URL ────────────────────────────────────────────────
        let url = if !input.url.is_empty() {
            input.url.clone()
        } else if let Some(ref p) = plugin {
            p.endpoint().to_string()
        } else {
            return Err(MyceliumError::Service(ServiceError::Unavailable(
                "no URL provided and no plugin endpoint available".to_string(),
            )));
        };

        // ── 3. Resolve query ──────────────────────────────────────────────
        let query = params["query"].as_str().ok_or_else(|| {
            MyceliumError::Service(ServiceError::InvalidResponse(
                "params.query is required".to_string(),
            ))
        })?;

        let operation_name = params["operation_name"].as_str();
        let mut variables = params["variables"].clone();
        if variables.is_null() {
            variables = json!({});
        }

        // ── 4. Resolve auth ───────────────────────────────────────────────
        let auth: Option<GraphQlAuth> = if !params["auth"].is_null() && params["auth"].is_object() {
            Self::parse_auth(&params["auth"])
        } else {
            plugin.as_ref().and_then(|p| p.default_auth())
        };

        // ── 5. Build headers (extra + plugin version headers) ─────────────
        let mut extra_headers: HashMap<String, String> = params["headers"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // Plugin version headers override ad-hoc ones for the same key
        if let Some(ref p) = plugin {
            for (k, v) in p.version_headers() {
                extra_headers.insert(k, v);
            }
        }

        // ── 6. Resolve pagination config ──────────────────────────────────
        let pag = &params["pagination"];
        let use_cursor = pag["strategy"].as_str() == Some("cursor");
        let page_info_path = pag["page_info_path"]
            .as_str()
            .unwrap_or("data.pageInfo")
            .to_string();
        let edges_path = pag["edges_path"]
            .as_str()
            .unwrap_or("data.edges")
            .to_string();
        let page_size: u64 = pag["page_size"]
            .as_u64()
            .unwrap_or_else(|| plugin.as_ref().map_or(50, |p| p.default_page_size() as u64));

        // ── 7. Execute (with optional cursor pagination) ───────────────────
        if use_cursor {
            // Inject the initial `first`/page-size variable and null cursor
            variables["first"] = json!(page_size);
            variables["after"] = json!(null);

            let mut all_edges: Vec<Value> = Vec::new();
            let mut page = 0usize;
            let mut cost_meta = json!(null);

            loop {
                if page >= self.config.max_pages {
                    return Err(MyceliumError::Service(ServiceError::InvalidResponse(
                        format!("pagination exceeded max_pages ({})", self.config.max_pages),
                    )));
                }

                let body = self
                    .post_query(
                        &url,
                        query,
                        &variables,
                        operation_name,
                        auth.as_ref(),
                        &extra_headers,
                    )
                    .await?;

                Self::validate_body(&body)?;

                // Accumulate edges
                let edges = Self::json_path(&body, &edges_path);
                if let Some(arr) = edges.as_array() {
                    all_edges.extend(arr.iter().cloned());
                }

                // Check for next page
                let page_info = Self::json_path(&body, &page_info_path);
                let has_next = page_info["hasNextPage"].as_bool().unwrap_or(false);
                let end_cursor = page_info["endCursor"].clone();

                cost_meta = Self::extract_cost_metadata(&body).unwrap_or(json!(null));
                page += 1;

                if !has_next || end_cursor.is_null() {
                    break;
                }
                variables["after"] = end_cursor;
            }

            let metadata = json!({ "cost": cost_meta, "pages_fetched": page });
            Ok(ServiceOutput {
                data: serde_json::to_string(&all_edges).unwrap_or_default(),
                metadata,
            })
        } else {
            // Single-request mode
            let body = self
                .post_query(
                    &url,
                    query,
                    &variables,
                    operation_name,
                    auth.as_ref(),
                    &extra_headers,
                )
                .await?;

            Self::validate_body(&body)?;

            let cost_meta = Self::extract_cost_metadata(&body).unwrap_or(json!(null));
            let metadata = json!({ "cost": cost_meta });

            Ok(ServiceOutput {
                data: serde_json::to_string(&body["data"]).unwrap_or_default(),
                metadata,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::field_reassign_with_default,
    clippy::unnecessary_literal_bound
)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Arc;

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::application::graphql_plugin_registry::GraphQlPluginRegistry;
    use crate::ports::graphql_plugin::GraphQlTargetPlugin;

    // ── Mock server ──────────────────────────────────────────────────────────

    /// Minimal HTTP/1.1 mock server that serves one fixed JSON response body.
    ///
    /// The server listens on a random port, serves one request, then stops.
    struct MockGraphQlServer;

    impl MockGraphQlServer {
        /// Spawn a server that returns HTTP `status` with `body` and run `f`.
        ///
        /// The closure receives the base URL `"http://127.0.0.1:<port>"`.
        async fn run_with<F, Fut>(status: u16, body: impl Into<Vec<u8>>, f: F)
        where
            F: FnOnce(String) -> Fut,
            Fut: std::future::Future<Output = ()>,
        {
            let body_bytes: Vec<u8> = body.into();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let url = format!("http://{addr}");

            let body_clone = body_bytes.clone();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf).await;
                    // Build a minimal HTTP/1.1 response
                    let mut response = Vec::new();
                    write!(
                        response,
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body_clone.len()
                    ).unwrap();
                    response.extend_from_slice(&body_clone);
                    let _ = stream.write_all(&response).await;
                }
            });

            f(url).await;
        }

        /// Variant that captures the received request headers for assertion.
        async fn run_capturing_request<F, Fut>(body: impl Into<Vec<u8>>, f: F) -> Vec<u8>
        where
            F: FnOnce(String) -> Fut,
            Fut: std::future::Future<Output = ()>,
        {
            let body_bytes: Vec<u8> = body.into();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let url = format!("http://{addr}");

            let body_clone = body_bytes.clone();
            let (tx, mut rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
            tokio::spawn(async move {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let request = buf[..n].to_vec();
                    let _ = tx.send(request);

                    let mut response = Vec::new();
                    write!(
                        response,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body_clone.len()
                    ).unwrap();
                    response.extend_from_slice(&body_clone);
                    let _ = stream.write_all(&response).await;
                }
            });

            f(url).await;

            rx.try_recv().unwrap_or_default()
        }
    }

    fn make_service(plugins: Option<Arc<GraphQlPluginRegistry>>) -> GraphQlService {
        let mut config = GraphQlConfig::default();
        config.max_pages = 5; // keep tests fast
        GraphQlService::new(config, plugins)
    }

    fn simple_query_body(data: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "data": data })).unwrap()
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_simple_query() {
        let body = simple_query_body(json!({ "users": [{ "id": 1 }] }));
        MockGraphQlServer::run_with(200, body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({ "query": "{ users { id } }" }),
            };
            let output = svc.execute(input).await.unwrap();
            let data: Value = serde_json::from_str(&output.data).unwrap();
            assert_eq!(data["users"][0]["id"], 1);
        })
        .await;
    }

    #[tokio::test]
    async fn graphql_errors_in_200_response() {
        let body =
            serde_json::to_vec(&json!({ "errors": [{ "message": "not found" }], "data": null }))
                .unwrap();
        MockGraphQlServer::run_with(200, body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({ "query": "{ missing }" }),
            };
            let err = svc.execute(input).await.unwrap_err();
            assert!(
                matches!(
                    err,
                    MyceliumError::Service(ServiceError::InvalidResponse(_))
                ),
                "expected InvalidResponse, got {err:?}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn http_error_returns_unavailable() {
        let body = b"Internal Server Error".to_vec();
        MockGraphQlServer::run_with(500, body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({ "query": "{ x }" }),
            };
            let err = svc.execute(input).await.unwrap_err();
            assert!(
                matches!(err, MyceliumError::Service(ServiceError::Unavailable(_))),
                "expected Unavailable, got {err:?}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn missing_data_key() {
        let body = serde_json::to_vec(&json!({ "extensions": {} })).unwrap();
        MockGraphQlServer::run_with(200, body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({ "query": "{ x }" }),
            };
            let err = svc.execute(input).await.unwrap_err();
            assert!(
                matches!(
                    err,
                    MyceliumError::Service(ServiceError::InvalidResponse(_))
                ),
                "expected InvalidResponse, got {err:?}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn bearer_auth_header_set() {
        let body = simple_query_body(json!({}));
        let request_bytes = MockGraphQlServer::run_capturing_request(body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({
                    "query": "{ x }",
                    "auth": { "kind": "bearer", "token": "test-token-123" }
                }),
            };
            let _ = svc.execute(input).await;
        })
        .await;

        let request_str = String::from_utf8_lossy(&request_bytes);
        assert!(
            request_str.contains("authorization: Bearer test-token-123"),
            "auth header not found in request:\n{request_str}"
        );
    }

    #[tokio::test]
    async fn plugin_version_headers_merged() {
        struct V1Plugin;
        impl GraphQlTargetPlugin for V1Plugin {
            fn name(&self) -> &str {
                "v1"
            }
            fn endpoint(&self) -> &str {
                "unused"
            }
            fn version_headers(&self) -> HashMap<String, String> {
                [("X-TEST-VERSION".to_string(), "2025-01-01".to_string())].into()
            }
        }

        let mut registry = GraphQlPluginRegistry::new();
        registry.register(Arc::new(V1Plugin));

        let body = simple_query_body(json!({}));
        let request_bytes = MockGraphQlServer::run_capturing_request(body, |url| async move {
            let svc = make_service(Some(Arc::new(registry)));
            let input = ServiceInput {
                url,
                params: json!({
                    "query": "{ x }",
                    "plugin": "v1"
                }),
            };
            let _ = svc.execute(input).await;
        })
        .await;

        let request_str = String::from_utf8_lossy(&request_bytes);
        assert!(
            request_str.contains("x-test-version: 2025-01-01"),
            "version header not found:\n{request_str}"
        );
    }

    #[tokio::test]
    async fn plugin_default_auth_used_when_params_auth_absent() {
        use crate::ports::{GraphQlAuth, GraphQlAuthKind};

        struct TokenPlugin;
        impl GraphQlTargetPlugin for TokenPlugin {
            fn name(&self) -> &str {
                "tokenplugin"
            }
            fn endpoint(&self) -> &str {
                "unused"
            }
            fn default_auth(&self) -> Option<GraphQlAuth> {
                Some(GraphQlAuth {
                    kind: GraphQlAuthKind::Bearer,
                    token: "plugin-default-token".to_string(),
                    header_name: None,
                })
            }
        }

        let mut registry = GraphQlPluginRegistry::new();
        registry.register(Arc::new(TokenPlugin));

        let body = simple_query_body(json!({}));
        let request_bytes = MockGraphQlServer::run_capturing_request(body, |url| async move {
            let svc = make_service(Some(Arc::new(registry)));
            let input = ServiceInput {
                url,
                // No `auth` field — plugin should supply it
                params: json!({
                    "query": "{ x }",
                    "plugin": "tokenplugin"
                }),
            };
            let _ = svc.execute(input).await;
        })
        .await;

        let request_str = String::from_utf8_lossy(&request_bytes);
        assert!(
            request_str.contains("Bearer plugin-default-token"),
            "plugin default auth not applied:\n{request_str}"
        );
    }

    #[tokio::test]
    async fn throttle_response_returns_rate_limited() {
        let body = serde_json::to_vec(&json!({
            "data": null,
            "extensions": {
                "cost": {
                    "throttleStatus": "THROTTLED",
                    "maximumAvailable": 10000,
                    "currentlyAvailable": 0,
                    "restoreRate": 500
                }
            }
        }))
        .unwrap();

        MockGraphQlServer::run_with(200, body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({ "query": "{ x }" }),
            };
            let err = svc.execute(input).await.unwrap_err();
            assert!(
                matches!(
                    err,
                    MyceliumError::Service(ServiceError::RateLimited { retry_after_ms })
                    if retry_after_ms > 0
                ),
                "expected RateLimited, got {err:?}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn cost_metadata_surfaced() {
        let body = serde_json::to_vec(&json!({
            "data": { "items": [] },
            "extensions": {
                "cost": {
                    "throttleStatus": "PASS",
                    "maximumAvailable": 10000,
                    "currentlyAvailable": 9800,
                    "actualQueryCost": 42,
                    "restoreRate": 500
                }
            }
        }))
        .unwrap();

        MockGraphQlServer::run_with(200, body, |url| async move {
            let svc = make_service(None);
            let input = ServiceInput {
                url,
                params: json!({ "query": "{ items { id } }" }),
            };
            let output = svc.execute(input).await.unwrap();
            let cost = &output.metadata["cost"];
            assert_eq!(cost["actualQueryCost"], 42);
            assert_eq!(cost["throttleStatus"], "PASS");
        })
        .await;
    }

    #[tokio::test]
    async fn cursor_pagination_accumulates_pages() {
        // Two-page scenario: page 1 has next page, page 2 does not.
        // We need two independent servers (one per page).
        let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr1 = listener1.local_addr().unwrap();
        let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr2 = listener2.local_addr().unwrap();

        // Both pages go to the same host:port — use a single server that handles
        // two sequential connections.
        let page1_body = serde_json::to_vec(&json!({
            "data": {
                "items": {
                    "edges": [{"node": {"id": 1}}, {"node": {"id": 2}}],
                    "pageInfo": { "hasNextPage": true, "endCursor": "cursor1" }
                }
            }
        }))
        .unwrap();

        let page2_body = serde_json::to_vec(&json!({
            "data": {
                "items": {
                    "edges": [{"node": {"id": 3}}],
                    "pageInfo": { "hasNextPage": false, "endCursor": null }
                }
            }
        }))
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let bodies = vec![page1_body, page2_body];
        tokio::spawn(async move {
            for response_body in bodies {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = [0u8; 8192];
                    let _ = stream.read(&mut buf).await;
                    let mut resp = Vec::new();
                    write!(
                        resp,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response_body.len()
                    ).unwrap();
                    resp.extend_from_slice(&response_body);
                    let _ = stream.write_all(&resp).await;
                }
            }
            // suppress unused warnings — listener1/2 and addr1/2 were created to
            // demonstrate the two-listener approach; the actual test uses a single listener
            let _ = listener1;
            let _ = listener2;
            let _ = addr1;
            let _ = addr2;
        });

        let svc = make_service(None);
        let input = ServiceInput {
            url,
            params: json!({
                "query": "query($first:Int,$after:String){ items(first:$first,after:$after){ edges{node{id}} pageInfo{hasNextPage endCursor} } }",
                "pagination": {
                    "strategy": "cursor",
                    "page_info_path": "data.items.pageInfo",
                    "edges_path": "data.items.edges",
                    "page_size": 2
                }
            }),
        };

        let output = svc.execute(input).await.unwrap();
        let edges: Vec<Value> = serde_json::from_str(&output.data).unwrap();
        assert_eq!(edges.len(), 3, "expected 3 accumulated edges");
        assert_eq!(edges[0]["node"]["id"], 1);
        assert_eq!(edges[2]["node"]["id"], 3);
    }

    #[tokio::test]
    async fn pagination_cap_prevents_infinite_loop() {
        // Every page reports hasNextPage=true — the cap should kick in.
        let page_body = serde_json::to_vec(&json!({
            "data": {
                "rows": {
                    "edges": [{"node": {"id": 1}}],
                    "pageInfo": { "hasNextPage": true, "endCursor": "always-more" }
                }
            }
        }))
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        let page_body_clone = page_body.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let mut resp = Vec::new();
                write!(
                    resp,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    page_body_clone.len()
                )
                .unwrap();
                resp.extend_from_slice(&page_body_clone);
                let _ = stream.write_all(&resp).await;
            }
        });

        // max_pages = 5 from make_service
        let svc = make_service(None);
        let input = ServiceInput {
            url,
            params: json!({
                "query": "{ rows { edges{node{id}} pageInfo{hasNextPage endCursor} } }",
                "pagination": {
                    "strategy": "cursor",
                    "page_info_path": "data.rows.pageInfo",
                    "edges_path": "data.rows.edges",
                    "page_size": 1
                }
            }),
        };

        let err = svc.execute(input).await.unwrap_err();
        assert!(
            matches!(err, MyceliumError::Service(ServiceError::InvalidResponse(ref msg)) if msg.contains("max_pages")),
            "expected pagination cap error, got {err:?}"
        );
    }
}
