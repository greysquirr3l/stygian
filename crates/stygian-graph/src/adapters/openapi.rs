//! OpenAPI 3.x introspection adapter.
//!
//! Implements [`crate::ports::ScrapingService`] for any API backed by an
//! OpenAPI 3.x specification (JSON or YAML).  At runtime the adapter:
//!
//! 1. Fetches and parses the spec, caching it for the lifetime of the adapter.
//! 2. Resolves the target operation by `operationId` or `"METHOD /path"`.
//! 3. Binds `params.args` to path parameters, query parameters, and request body.
//! 4. Delegates the concrete HTTP call to the inner [`crate::adapters::rest_api::RestApiAdapter`].
//!
//! An optional proactive rate limit (`params.rate_limit`) is enforced before
//! each request; reactive 429 handling is inherited from [`crate::adapters::rest_api::RestApiAdapter`].
//!
//! # `ServiceInput` contract
//!
//! | Field | Type | Description |
//! |-------|------|-------------|
//! | `url` | string | URL of the OpenAPI spec (JSON or YAML) |
//! | `params.operation` | string | `operationId` **or** `"METHOD /path"` |
//! | `params.args` | object | Path / query / body args (merged) |
//! | `params.auth` | object | Same shape as [`crate::adapters::rest_api::RestApiAdapter`] |
//! | `params.server.url` | string | Override the spec's `servers[0].url` |
//! | `params.rate_limit` | object | Optional proactive throttle |
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::openapi::OpenApiAdapter;
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = OpenApiAdapter::new();
//!
//! let input = ServiceInput {
//!     url: "https://petstore3.swagger.io/api/v3/openapi.json".to_string(),
//!     params: json!({
//!         "operation": "listPets",
//!         "args": { "status": "available" },
//!         "auth": { "type": "api_key_header", "header": "api_key", "key": "special-key" },
//!     }),
//! };
//! // let output = adapter.execute(input).await.unwrap();
//! # });
//! ```

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use openapiv3::{OpenAPI, Operation, Parameter, ReferenceOr};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::adapters::graphql_rate_limit::{
    RateLimitConfig, RateLimitStrategy, RequestRateLimit, rate_limit_acquire,
};
use crate::adapters::rest_api::{RestApiAdapter, RestApiConfig};
use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Spec cache ───────────────────────────────────────────────────────────────

type SpecCache = Arc<RwLock<HashMap<String, Arc<OpenAPI>>>>;

// ─── Config ───────────────────────────────────────────────────────────────────

/// Configuration for [`OpenApiAdapter`].
///
/// Adapter-level defaults; per-request settings come from `ServiceInput.params`.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::openapi::OpenApiConfig;
/// use stygian_graph::adapters::rest_api::RestApiConfig;
/// use std::time::Duration;
///
/// let config = OpenApiConfig {
///     rest: RestApiConfig {
///         timeout:      Duration::from_secs(20),
///         max_retries:  2,
///         ..Default::default()
///     },
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct OpenApiConfig {
    /// Config forwarded to the inner [`RestApiAdapter`].
    pub rest: RestApiConfig,
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// `OpenAPI 3.x` introspection adapter.
///
/// Thread-safe and cheaply cloneable — the inner `reqwest::Client` and the
/// spec cache both use `Arc` internally.  Build once, share across tasks.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::openapi::OpenApiAdapter;
///
/// let adapter = OpenApiAdapter::new();
/// ```
#[derive(Clone)]
pub struct OpenApiAdapter {
    /// Inner REST adapter — handles all actual HTTP calls.
    inner: RestApiAdapter,
    /// HTTP client used exclusively to fetch `OpenAPI` spec documents.
    spec_client: Client,
    /// Parsed specs keyed by their fetch `URL`.
    spec_cache: SpecCache,
    /// Lazily initialised proactive rate limiter, seeded from `params.rate_limit`
    /// on the first call.  Shared across all clones of this adapter.
    rate_limit: Arc<OnceLock<RequestRateLimit>>,
}

impl OpenApiAdapter {
    /// Create a new adapter with default configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::openapi::OpenApiAdapter;
    /// let adapter = OpenApiAdapter::new();
    /// ```
    pub fn new() -> Self {
        Self::with_config(OpenApiConfig::default())
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
    /// use stygian_graph::adapters::openapi::{OpenApiAdapter, OpenApiConfig};
    /// use stygian_graph::adapters::rest_api::RestApiConfig;
    /// use std::time::Duration;
    ///
    /// let adapter = OpenApiAdapter::with_config(OpenApiConfig {
    ///     rest: RestApiConfig {
    ///         timeout: Duration::from_secs(10),
    ///         ..Default::default()
    ///     },
    /// });
    /// ```
    pub fn with_config(config: OpenApiConfig) -> Self {
        // SAFETY: TLS via rustls is always available.
        #[allow(clippy::expect_used)]
        let spec_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .use_rustls_tls()
            .build()
            .expect("TLS backend unavailable");

        Self {
            inner: RestApiAdapter::with_config(config.rest),
            spec_client,
            spec_cache: Arc::new(RwLock::new(HashMap::new())),
            rate_limit: Arc::new(OnceLock::new()),
        }
    }
}

impl Default for OpenApiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Wrap an error message as [`ServiceError::Unavailable`].
fn svc_err(msg: impl Into<String>) -> StygianError {
    StygianError::from(ServiceError::Unavailable(msg.into()))
}

/// Fetch and parse an `OpenAPI` spec from `url`.
///
/// Tries `JSON` first (fast path), then falls back to `YAML`.
async fn fetch_spec(client: &Client, url: &str) -> Result<Arc<OpenAPI>> {
    let body = client
        .get(url)
        .header(
            "Accept",
            "application/json, application/yaml, text/yaml, */*",
        )
        .send()
        .await
        .map_err(|e| svc_err(format!("spec fetch failed: {e}")))?
        .text()
        .await
        .map_err(|e| svc_err(format!("spec read failed: {e}")))?;

    let api: OpenAPI = serde_json::from_str(&body)
        .or_else(|_| serde_yaml::from_str(&body))
        .map_err(|e| svc_err(format!("spec parse failed: {e}")))?;

    Ok(Arc::new(api))
}

/// Return the spec for `url`, using the cache when available.
async fn resolve_spec(cache: &SpecCache, client: &Client, url: &str) -> Result<Arc<OpenAPI>> {
    {
        let guard = cache.read().await;
        if let Some(spec) = guard.get(url) {
            debug!(url, "OpenAPI spec cache hit");
            return Ok(Arc::clone(spec));
        }
    }

    // Fetch outside the lock to avoid blocking concurrent readers.
    let spec = fetch_spec(client, url).await?;

    {
        let mut guard = cache.write().await;
        // A concurrent task may have inserted the same spec; prefer that entry.
        guard
            .entry(url.to_owned())
            .or_insert_with(|| Arc::clone(&spec));
    }

    Ok(spec)
}

/// Resolve an operation from the spec.
///
/// `operation_ref` is either an `operationId` (e.g. `"listPets"`) or a
/// `"METHOD /path"` string (e.g. `"GET /pets"`).
///
/// Returns `(http_method, path_template, operation)`.
fn resolve_operation<'a>(
    api: &'a OpenAPI,
    operation_ref: &str,
) -> Result<(String, String, &'a Operation)> {
    // Pre-parse "METHOD /path" format so we avoid splitting in the inner loop.
    let method_path: Option<(String, &str)> = operation_ref
        .split_once(' ')
        .filter(|(m, _)| {
            matches!(
                m.to_uppercase().as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
            )
        })
        .map(|(m, p)| (m.to_uppercase(), p));

    for (path_str, path_item_ref) in &api.paths.paths {
        let item = match path_item_ref {
            ReferenceOr::Item(i) => i,
            ReferenceOr::Reference { .. } => continue,
        };

        let ops: [(&str, Option<&Operation>); 8] = [
            ("GET", item.get.as_ref()),
            ("POST", item.post.as_ref()),
            ("PUT", item.put.as_ref()),
            ("PATCH", item.patch.as_ref()),
            ("DELETE", item.delete.as_ref()),
            ("HEAD", item.head.as_ref()),
            ("OPTIONS", item.options.as_ref()),
            ("TRACE", item.trace.as_ref()),
        ];

        for (method, maybe_op) in ops {
            let Some(op) = maybe_op else { continue };

            let matched = match &method_path {
                Some((target_method, target_path)) => {
                    method == target_method.as_str() && path_str == target_path
                }
                None => op.operation_id.as_deref() == Some(operation_ref),
            };

            if matched {
                return Ok((method.to_owned(), path_str.clone(), op));
            }
        }
    }

    Err(svc_err(format!(
        "operation '{operation_ref}' not found in spec"
    )))
}

/// Select the effective server base URL.
///
/// Priority: `params.server.url` → first server in spec → empty string.
#[allow(clippy::indexing_slicing)]
fn resolve_server(api: &OpenAPI, server_override: &Value) -> String {
    if let Some(url) = server_override.as_str().filter(|s| !s.is_empty()) {
        return url.trim_end_matches('/').to_owned();
    }
    api.servers
        .first()
        .map(|s| s.url.trim_end_matches('/').to_owned())
        .unwrap_or_default()
}

/// Partition the operation's declared parameters into path and query name lists.
fn classify_params(op: &Operation) -> (Vec<String>, Vec<String>) {
    let mut path_params: Vec<String> = Vec::new();
    let mut query_params: Vec<String> = Vec::new();

    for p_ref in &op.parameters {
        let p = match p_ref {
            ReferenceOr::Item(p) => p,
            ReferenceOr::Reference { .. } => continue,
        };
        match p {
            Parameter::Path { parameter_data, .. } => {
                path_params.push(parameter_data.name.clone());
            }
            Parameter::Query { parameter_data, .. } => {
                query_params.push(parameter_data.name.clone());
            }
            // Header and Cookie params are uncommon; skip (not a correctness issue
            // for the request — the caller can pass them via `params.auth` or custom headers).
            Parameter::Header { .. } | Parameter::Cookie { .. } => {}
        }
    }

    (path_params, query_params)
}

/// Substitute `{param}` placeholders in `path_template` using `args`.
fn build_url(server_url: &str, path_template: &str, args: &HashMap<String, Value>) -> String {
    let mut url = format!("{server_url}{path_template}");
    for (key, val) in args {
        let placeholder = format!("{{{key}}}");
        if url.contains(placeholder.as_str()) {
            let replacement = val.as_str().map_or_else(|| val.to_string(), str::to_owned);
            url = url.replace(placeholder.as_str(), &replacement);
        }
    }
    url
}

/// Build the `params` object consumed by the inner [`RestApiAdapter`].
///
/// - `args` keys that match `path_param_names` are already substituted into the URL.
/// - `args` keys that match `query_param_names` are placed in `params.query`.
/// - Remaining `args` (when the operation declares a requestBody) go into `params.body`.
#[allow(clippy::indexing_slicing)]
fn build_rest_params(
    method: &str,
    op: &Operation,
    args: &HashMap<String, Value>,
    path_param_names: &[String],
    query_param_names: &[String],
    auth_override: &Value,
) -> Value {
    let query_obj: serde_json::Map<String, Value> = query_param_names
        .iter()
        .filter_map(|name| {
            args.get(name.as_str()).map(|val| {
                let s = val.as_str().map_or_else(|| val.to_string(), str::to_owned);
                (name.clone(), Value::String(s))
            })
        })
        .collect();

    let body_value = if op.request_body.is_some() {
        let excluded: std::collections::HashSet<&str> = path_param_names
            .iter()
            .chain(query_param_names.iter())
            .map(String::as_str)
            .collect();
        let body_args: serde_json::Map<String, Value> = args
            .iter()
            .filter(|(k, _)| !excluded.contains(k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if body_args.is_empty() {
            Value::Null
        } else {
            Value::Object(body_args)
        }
    } else {
        Value::Null
    };

    let mut params = json!({
        "method": method,
        "query":  Value::Object(query_obj),
    });

    if !body_value.is_null() {
        params["body"] = body_value;
    }
    if !auth_override.is_null() {
        params["auth"] = auth_override.clone();
    }

    params
}

/// Parse a `params.rate_limit` JSON object into a [`RateLimitConfig`].
#[allow(clippy::indexing_slicing)]
fn parse_rate_limit_config(rl: &Value) -> RateLimitConfig {
    let strategy = match rl["strategy"].as_str().unwrap_or("sliding_window") {
        "token_bucket" => RateLimitStrategy::TokenBucket,
        _ => RateLimitStrategy::SlidingWindow,
    };
    RateLimitConfig {
        max_requests: rl["max_requests"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(100),
        window: Duration::from_secs(rl["window_secs"].as_u64().unwrap_or(60)),
        max_delay_ms: rl["max_delay_ms"].as_u64().unwrap_or(30_000),
        strategy,
    }
}

// ─── ScrapingService ──────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for OpenApiAdapter {
    /// Execute an `OpenAPI` operation and return the result.
    ///
    /// # `ServiceInput.url`
    ///
    /// `URL` of the `OpenAPI` specification document (`.json` or `.yaml`).
    ///
    /// # `ServiceInput.params` contract
    ///
    /// ```json
    /// {
    ///   "operation": "listPets",
    ///   "args": {
    ///     "status":  "available",
    ///     "petId":   42
    ///   },
    ///   "auth": {
    ///     "type":   "api_key_header",
    ///     "header": "api_key",
    ///     "key":    "my-secret"
    ///   },
    ///   "server": {
    ///     "url": "https://override.example.com/v1"
    ///   },
    ///   "rate_limit": {
    ///     "max_requests": 100,
    ///     "window_secs":  60,
    ///     "strategy":     "token_bucket"
    ///   }
    /// }
    /// ```
    ///
    /// # Rate limiting
    ///
    /// Two independent layers operate simultaneously:
    ///
    /// 1. **Proactive** — `params.rate_limit` (optional, token-bucket or sliding-window).
    ///    Enforced before each request by sleeping until a slot is available.
    /// 2. **Reactive** — inherited from the inner [`RestApiAdapter`].  A `429` response
    ///    with a `Retry-After` header causes an automatic sleep and retry.
    #[allow(clippy::indexing_slicing)]
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        // ── Proactive rate limit ─────────────────────────────────────────────
        let rl_params = &input.params["rate_limit"];
        if !rl_params.is_null() {
            let rl = self
                .rate_limit
                .get_or_init(|| RequestRateLimit::new(parse_rate_limit_config(rl_params)));
            rate_limit_acquire(rl).await;
        }

        info!(url = %input.url, "OpenAPI adapter: execute");

        // ── Resolve spec ─────────────────────────────────────────────────────
        let api = resolve_spec(&self.spec_cache, &self.spec_client, &input.url).await?;

        // ── Resolve operation ────────────────────────────────────────────────
        let operation_ref = input.params["operation"]
            .as_str()
            .ok_or_else(|| svc_err("params.operation is required"))?;

        let (method, path_template, op) = resolve_operation(&api, operation_ref)?;

        // ── Server URL ───────────────────────────────────────────────────────
        let server_url = resolve_server(&api, &input.params["server"]["url"]);

        // ── Parameter classification ─────────────────────────────────────────
        let (path_param_names, query_param_names) = classify_params(op);

        // ── Collect args ─────────────────────────────────────────────────────
        let args: HashMap<String, Value> = input.params["args"]
            .as_object()
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        // ── Build resolved URL ────────────────────────────────────────────────
        let final_url = build_url(&server_url, &path_template, &args);

        // ── Build inner params ────────────────────────────────────────────────
        let rest_params = build_rest_params(
            &method,
            op,
            &args,
            &path_param_names,
            &query_param_names,
            &input.params["auth"],
        );

        debug!(
            %final_url, %method, path_template, operation_ref,
            "OpenAPI: delegating to RestApiAdapter"
        );

        // ── Delegate ─────────────────────────────────────────────────────────
        let inner_output = self
            .inner
            .execute(ServiceInput {
                url: final_url.clone(),
                params: rest_params,
            })
            .await?;

        // ── Augment metadata ──────────────────────────────────────────────────
        let mut metadata = inner_output.metadata;
        if let Value::Object(ref mut m) = metadata {
            m.insert(
                "openapi_spec_url".to_owned(),
                Value::String(input.url.clone()),
            );
            m.insert(
                "operation_id".to_owned(),
                Value::String(operation_ref.to_owned()),
            );
            m.insert("method".to_owned(), Value::String(method));
            m.insert("path_template".to_owned(), Value::String(path_template));
            m.insert("server_url".to_owned(), Value::String(server_url));
            m.insert("resolved_url".to_owned(), Value::String(final_url));
        }

        Ok(ServiceOutput {
            data: inner_output.data,
            metadata,
        })
    }

    fn name(&self) -> &'static str {
        "openapi"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::expect_used
)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    // ── Minimal embedded spec ─────────────────────────────────────────────────

    /// A self-contained Petstore-style spec used in all unit tests.
    const MINI_SPEC: &str = r#"{
      "openapi": "3.0.0",
      "info": { "title": "Mini Test API", "version": "1.0" },
      "servers": [{ "url": "https://api.example.com/v1" }],
      "paths": {
        "/pets": {
          "get": {
            "operationId": "listPets",
            "parameters": [
              { "name": "limit",  "in": "query", "schema": { "type": "integer" } },
              { "name": "status", "in": "query", "schema": { "type": "string"  } }
            ],
            "responses": { "200": { "description": "OK" } }
          }
        },
        "/pets/{petId}": {
          "get": {
            "operationId": "getPet",
            "parameters": [
              { "name": "petId", "in": "path", "required": true, "schema": { "type": "integer" } }
            ],
            "responses": { "200": { "description": "OK" } }
          },
          "delete": {
            "operationId": "deletePet",
            "parameters": [
              { "name": "petId", "in": "path", "required": true, "schema": { "type": "integer" } }
            ],
            "responses": { "204": { "description": "No content" } }
          }
        },
        "/pets/findByStatus": {
          "get": {
            "operationId": "findPetsByStatus",
            "parameters": [
              { "name": "status", "in": "query", "schema": { "type": "string" } }
            ],
            "responses": { "200": { "description": "OK" } }
          }
        }
      },
      "components": {
        "securitySchemes": {
          "apiKeyAuth": { "type": "apiKey", "in": "header", "name": "X-Api-Key" }
        }
      }
    }"#;

    fn parse_mini() -> Arc<OpenAPI> {
        Arc::new(serde_json::from_str(MINI_SPEC).expect("MINI_SPEC is valid JSON"))
    }

    // ── 1. parse_petstore_spec ────────────────────────────────────────────────

    #[test]
    fn parse_petstore_spec() {
        let api = parse_mini();
        assert_eq!(api.paths.paths.len(), 3, "spec has 3 paths");
        assert!(api.components.is_some());
    }

    // ── 2. resolve_operation_by_id ────────────────────────────────────────────

    #[test]
    fn resolve_operation_by_id() {
        let api = parse_mini();
        let (method, path, op) = resolve_operation(&api, "listPets").unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/pets");
        assert_eq!(op.operation_id.as_deref(), Some("listPets"));
    }

    // ── 3. resolve_operation_by_method_path ──────────────────────────────────

    #[test]
    fn resolve_operation_by_method_path() {
        let api = parse_mini();
        let (method, path, op) = resolve_operation(&api, "GET /pets/findByStatus").unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/pets/findByStatus");
        assert_eq!(op.operation_id.as_deref(), Some("findPetsByStatus"));
    }

    // ── 4. resolve_operation_not_found ───────────────────────────────────────

    #[test]
    fn resolve_operation_not_found() {
        let api = parse_mini();
        assert!(resolve_operation(&api, "nonExistentOp").is_err());
    }

    // ── 5. bind_path_params ───────────────────────────────────────────────────

    #[test]
    fn bind_path_params() {
        let args: HashMap<String, Value> = HashMap::from([("petId".to_owned(), json!(42))]);
        let url = build_url("https://api.example.com/v1", "/pets/{petId}", &args);
        assert_eq!(url, "https://api.example.com/v1/pets/42");
    }

    #[test]
    fn bind_path_params_string() {
        let args: HashMap<String, Value> = HashMap::from([("petId".to_owned(), json!("fluffy"))]);
        let url = build_url("https://api.example.com/v1", "/pets/{petId}", &args);
        assert_eq!(url, "https://api.example.com/v1/pets/fluffy");
    }

    // ── 6. bind_query_params ──────────────────────────────────────────────────

    #[test]
    fn bind_query_params() {
        let api = parse_mini();
        let (_, _, op) = resolve_operation(&api, "listPets").unwrap();
        let (path_names, query_names) = classify_params(op);
        assert!(path_names.is_empty());
        assert!(query_names.contains(&"status".to_owned()));
        assert!(query_names.contains(&"limit".to_owned()));

        let args: HashMap<String, Value> = [
            ("status".to_owned(), json!("available")),
            ("limit".to_owned(), json!("10")),
        ]
        .into_iter()
        .collect();

        let params = build_rest_params("GET", op, &args, &path_names, &query_names, &Value::Null);
        assert_eq!(params["query"]["status"], json!("available"));
        assert_eq!(params["query"]["limit"], json!("10"));
    }

    // ── 7. server_override ───────────────────────────────────────────────────

    #[test]
    fn server_override() {
        let api = parse_mini();
        let url = resolve_server(&api, &json!("https://override.example.com/v2/"));
        assert_eq!(url, "https://override.example.com/v2");

        let default_url = resolve_server(&api, &Value::Null);
        assert_eq!(default_url, "https://api.example.com/v1");
    }

    // ── 8. spec_cache_hit ────────────────────────────────────────────────────
    // Tested indirectly: build a SpecCache, manually pre-populate it, then call
    // resolve_spec with a URL that is already present.  Confirm the same Arc
    // is returned.

    #[tokio::test]
    async fn spec_cache_hit() {
        let cache: SpecCache = Arc::new(RwLock::new(HashMap::new()));

        // Pre-warm using the raw client path.
        let api = parse_mini();
        cache
            .write()
            .await
            .insert("http://test/spec.json".to_owned(), Arc::clone(&api));

        // resolve_spec should return the cached entry without making an HTTP call.
        // Using a fake client that would panic if used — the cache hit skips fetching.
        #[allow(clippy::expect_used)]
        let dummy_client = Client::builder().use_rustls_tls().build().expect("client");

        let returned = resolve_spec(&cache, &dummy_client, "http://test/spec.json")
            .await
            .unwrap();

        // Both Arcs should point to the same allocation.
        assert!(Arc::ptr_eq(&api, &returned));
    }

    // ── 9. rate_limit_proactive ──────────────────────────────────────────────

    #[tokio::test]
    async fn rate_limit_proactive() {
        use crate::adapters::graphql_rate_limit::rate_limit_acquire;
        use tokio::time::Instant;

        let config = RateLimitConfig {
            max_requests: 3,
            window: Duration::from_secs(10),
            max_delay_ms: 5_000,
            strategy: RateLimitStrategy::SlidingWindow,
        };
        let rl = RequestRateLimit::new(config);

        // First 3 requests should pass immediately.
        for _ in 0..3 {
            rate_limit_acquire(&rl).await;
        }

        // 4th request must block.  We assert it takes > 0 ms (i.e. a delay was imposed).
        let start = Instant::now();
        // Use a very short window so the test doesn't wait 10 s.
        let config_short = RateLimitConfig {
            max_requests: 1,
            window: Duration::from_millis(50),
            max_delay_ms: 200,
            strategy: RateLimitStrategy::SlidingWindow,
        };
        let rl_short = RequestRateLimit::new(config_short);
        rate_limit_acquire(&rl_short).await; // slot 1
        rate_limit_acquire(&rl_short).await; // slot 2 — must sleep ≥ 50 ms
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(40),
            "expected ≥40 ms delay but got {elapsed:?}"
        );
    }

    // ── 10. parse_rate_limit_config ──────────────────────────────────────────

    #[test]
    fn parse_rate_limit_config_token_bucket() {
        let rl = json!({
            "max_requests": 50,
            "window_secs":  30,
            "strategy":     "token_bucket",
        });
        let cfg = parse_rate_limit_config(&rl);
        assert_eq!(cfg.max_requests, 50);
        assert_eq!(cfg.window, Duration::from_secs(30));
        assert_eq!(cfg.strategy, RateLimitStrategy::TokenBucket);
    }

    #[test]
    fn parse_rate_limit_config_defaults() {
        let cfg = parse_rate_limit_config(&json!({}));
        assert_eq!(cfg.max_requests, 100);
        assert_eq!(cfg.window, Duration::from_mins(1));
        assert_eq!(cfg.strategy, RateLimitStrategy::SlidingWindow);
    }

    // ── 11. adapter name ─────────────────────────────────────────────────────

    #[test]
    fn adapter_name() {
        assert_eq!(OpenApiAdapter::new().name(), "openapi");
    }
}
