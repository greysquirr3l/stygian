//! REST API server for pipeline management (T30)
//!
//! Provides an HTTP API for submitting, monitoring, and managing scraping
//! pipelines. Includes a built-in web dashboard served at `/` (T31).
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! | -------- | ------ | ------------- |
//! | `GET` | `/health` | Liveness probe |
//! | `GET` | `/metrics` | Prometheus metrics |
//! | `GET` | `/` | Web dashboard (HTML) |
//! | `POST` | `/pipelines` | Submit a new pipeline |
//! | `GET` | `/pipelines` | List all pipelines |
//! | `GET` | `/pipelines/:id` | Get pipeline status |
//! | `GET` | `/pipelines/:id/results` | Get pipeline results |
//! | `DELETE` | `/pipelines/:id` | Cancel / delete a pipeline |
//!
//! # Authentication
//!
//! All `/pipelines` routes require an `X-Api-Key` header.  Set the API key via
//! the `STYGIAN_API_KEY` environment variable (defaults to `"dev-key"` when
//! the variable is not set).
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::application::api_server::ApiServer;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let server = ApiServer::from_env();
//!     server.run("0.0.0.0:8080").await
//! }
//! ```

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Domain types
// ─────────────────────────────────────────────────────────────────────────────

/// Current execution state of a submitted pipeline.
///
/// # Example
///
/// ```
/// use stygian_graph::application::api_server::PipelineState;
///
/// let state = PipelineState::Pending;
/// assert!(matches!(state, PipelineState::Pending));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PipelineState {
    /// Queued, waiting for an executor
    Pending,
    /// Currently executing
    Running,
    /// Finished successfully
    Completed,
    /// Finished with an error
    Failed,
    /// Cancelled by the user
    Cancelled,
}

/// A pipeline run record stored in the in-memory registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    /// Unique identifier (`UUIDv4`)
    pub id: String,
    /// User-supplied pipeline definition (TOML or JSON)
    pub definition: Value,
    /// Current state
    pub state: PipelineState,
    /// Unix timestamp (seconds) when the pipeline was submitted
    pub submitted_at: u64,
    /// Unix timestamp (seconds) when the pipeline finished, if applicable
    pub finished_at: Option<u64>,
    /// Accumulated results (`node_name` → output)
    pub results: Value,
    /// Error message if `state == Failed`
    pub error: Option<String>,
}

impl PipelineRun {
    fn new(id: String, definition: Value) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self {
            id,
            definition,
            state: PipelineState::Pending,
            submitted_at: now,
            finished_at: None,
            results: json!({}),
            error: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Request / response shapes
// ─────────────────────────────────────────────────────────────────────────────

/// Request body for `POST /pipelines`
#[derive(Debug, Deserialize)]
pub struct SubmitPipelineRequest {
    /// Pipeline definition (TOML string or structured JSON)
    pub definition: Value,
}

/// Response from `POST /pipelines`
#[derive(Debug, Serialize)]
pub struct SubmitPipelineResponse {
    /// Assigned pipeline ID
    pub id: String,
    /// Initial state (always `pending`)
    pub state: PipelineState,
}

/// Slim status summary returned by `GET /pipelines` and `GET /pipelines/:id`
#[derive(Debug, Serialize)]
pub struct PipelineStatus {
    /// Pipeline ID
    pub id: String,
    /// Current state
    pub state: PipelineState,
    /// Submission timestamp (Unix seconds)
    pub submitted_at: u64,
    /// Completion timestamp (Unix seconds), if finished
    pub finished_at: Option<u64>,
    /// Error message, if failed
    pub error: Option<String>,
}

impl From<&PipelineRun> for PipelineStatus {
    fn from(r: &PipelineRun) -> Self {
        Self {
            id: r.id.clone(),
            state: r.state.clone(),
            submitted_at: r.submitted_at,
            finished_at: r.finished_at,
            error: r.error.clone(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared application state
// ─────────────────────────────────────────────────────────────────────────────

/// Shared state injected into every route handler.
#[derive(Clone)]
pub struct AppState {
    /// In-memory pipeline registry
    pub pipelines: Arc<DashMap<String, PipelineRun>>,
    /// API key required for /pipelines routes
    pub api_key: String,
}

impl AppState {
    /// Create state with the given API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            pipelines: Arc::new(DashMap::new()),
            api_key: api_key.into(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Auth middleware
// ─────────────────────────────────────────────────────────────────────────────

/// Middleware that enforces `X-Api-Key` authentication.
///
/// Requests missing or carrying a wrong key receive a `401 Unauthorized`
/// response and never reach the protected route.
async fn require_api_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    let provided = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided != state.api_key {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid or missing X-Api-Key"})),
        )
            .into_response();
    }
    next.run(request).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Route handlers
// ─────────────────────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "stygian-api",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn metrics() -> impl IntoResponse {
    // Return empty Prometheus metrics response; a full integration would
    // call the MetricsRegistry::render() from application::metrics.
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        "# stygian-api metrics\n",
    )
}

async fn dashboard() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        DASHBOARD_HTML,
    )
}

async fn submit_pipeline(
    State(state): State<AppState>,
    Json(body): Json<SubmitPipelineRequest>,
) -> impl IntoResponse {
    let id = Uuid::new_v4().to_string();
    let run = PipelineRun::new(id.clone(), body.definition);
    state.pipelines.insert(id.clone(), run);
    info!(pipeline_id = %id, "pipeline submitted");
    (
        StatusCode::CREATED,
        Json(SubmitPipelineResponse {
            id,
            state: PipelineState::Pending,
        }),
    )
}

async fn list_pipelines(State(state): State<AppState>) -> impl IntoResponse {
    let list: Vec<PipelineStatus> = state
        .pipelines
        .iter()
        .map(|e| PipelineStatus::from(e.value()))
        .collect();
    Json(list)
}

#[allow(clippy::option_if_let_else)]
async fn get_pipeline(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.pipelines.get(&id) {
        Some(run) => Json(PipelineStatus::from(run.value())).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "pipeline not found"})),
        )
            .into_response(),
    }
}

async fn get_pipeline_results(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.pipelines.get(&id) {
        Some(run) => {
            if run.state == PipelineState::Completed {
                Json(json!({
                    "id": run.id,
                    "results": run.results,
                }))
                .into_response()
            } else {
                (
                    StatusCode::ACCEPTED,
                    Json(json!({
                        "id": run.id,
                        "state": run.state,
                        "message": "pipeline not yet complete",
                    })),
                )
                    .into_response()
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "pipeline not found"})),
        )
            .into_response(),
    }
}

async fn cancel_pipeline(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.pipelines.remove(&id) {
        Some(_) => {
            info!(pipeline_id = %id, "pipeline cancelled/deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "pipeline not found"})),
        )
            .into_response(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Router assembly
// ─────────────────────────────────────────────────────────────────────────────

/// Build the axum [`Router`] with all routes and middleware attached.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::application::api_server::{build_router, AppState};
///
/// let state = AppState::new("my-secret-key");
/// let app = build_router(state);
/// ```
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/pipelines", post(submit_pipeline).get(list_pipelines))
        .route("/pipelines/{id}", get(get_pipeline).delete(cancel_pipeline))
        .route("/pipelines/{id}/results", get(get_pipeline_results))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ));

    let public = Router::new()
        .route("/", get(dashboard))
        .route("/health", get(health))
        .route("/metrics", get(metrics));

    Router::new()
        .merge(public)
        .merge(protected)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ─────────────────────────────────────────────────────────────────────────────
// ApiServer
// ─────────────────────────────────────────────────────────────────────────────

/// High-level API server wrapper.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::application::api_server::ApiServer;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let server = ApiServer::from_env();
///     server.run("127.0.0.1:8080").await
/// }
/// ```
pub struct ApiServer {
    state: AppState,
}

impl ApiServer {
    /// Create an `ApiServer` with a specific API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            state: AppState::new(api_key),
        }
    }

    /// Create an `ApiServer` from environment variables.
    ///
    /// Reads `STYGIAN_API_KEY` (defaults to `"dev-key"` if unset).
    pub fn from_env() -> Self {
        let key = std::env::var("STYGIAN_API_KEY").unwrap_or_else(|_| "dev-key".to_string());
        Self::new(key)
    }

    /// Start listening on `addr` and serve requests until the process is
    /// killed.
    ///
    /// # Errors
    ///
    /// Returns an error if the address cannot be bound.
    pub async fn run(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let app = build_router(self.state);
        let listener = TcpListener::bind(addr).await?;
        info!(address = %addr, "stygian-api listening");
        axum::serve(listener, app).await?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Web dashboard HTML (T31)
// ─────────────────────────────────────────────────────────────────────────────

/// Embedded web dashboard served at `GET /`.
///
/// Uses Tailwind CSS (CDN) and vanilla `fetch()` calls against the REST API.
/// No build step required — ships as a single HTML constant.
const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Stygian Dashboard</title>
  <script src="https://cdn.tailwindcss.com"></script>
  <style>
    body { font-family: 'Inter', system-ui, sans-serif; }
    .badge-pending    { @apply bg-yellow-100 text-yellow-800; }
    .badge-running    { @apply bg-blue-100 text-blue-800; }
    .badge-completed  { @apply bg-green-100 text-green-800; }
    .badge-failed     { @apply bg-red-100 text-red-800; }
    .badge-cancelled  { @apply bg-gray-100 text-gray-800; }
  </style>
</head>
<body class="bg-gray-50 text-gray-900 min-h-screen">

<!-- Nav -->
<nav class="bg-indigo-700 text-white px-6 py-4 flex items-center gap-3 shadow-md">
  <span class="text-2xl">🕸️</span>
  <h1 class="text-xl font-bold tracking-tight">Stygian</h1>
  <span class="ml-auto text-sm opacity-70">Pipeline Dashboard</span>
</nav>

<!-- Main -->
<main class="max-w-5xl mx-auto px-4 py-8 space-y-8">

  <!-- Health card -->
  <section class="bg-white rounded-xl shadow p-6">
    <h2 class="text-lg font-semibold mb-3">System Health</h2>
    <div id="health" class="text-sm text-gray-500">Loading…</div>
  </section>

  <!-- Submit pipeline -->
  <section class="bg-white rounded-xl shadow p-6 space-y-4">
    <h2 class="text-lg font-semibold">Submit Pipeline</h2>
    <div class="space-y-2">
      <label class="text-sm font-medium text-gray-700">API Key</label>
      <input id="apikey" type="password" placeholder="X-Api-Key value"
        class="w-full border border-gray-300 rounded-lg px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-500" />
    </div>
    <div class="space-y-2">
      <label class="text-sm font-medium text-gray-700">Pipeline definition (JSON)</label>
      <textarea id="pipelineDef" rows="6" placeholder='{"nodes":[]}'
        class="w-full border border-gray-300 rounded-lg px-3 py-2 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-indigo-500"></textarea>
    </div>
    <button onclick="submitPipeline()"
      class="bg-indigo-600 hover:bg-indigo-700 text-white px-5 py-2 rounded-lg text-sm font-semibold transition-colors">
      Submit
    </button>
    <div id="submit-result" class="text-sm"></div>
  </section>

  <!-- Pipeline list -->
  <section class="bg-white rounded-xl shadow p-6">
    <div class="flex items-center justify-between mb-4">
      <h2 class="text-lg font-semibold">Pipelines</h2>
      <button onclick="loadPipelines()"
        class="text-sm text-indigo-600 hover:text-indigo-800 font-medium">Refresh</button>
    </div>
    <div id="pipeline-list" class="space-y-2 text-sm text-gray-500">Loading…</div>
  </section>

</main>

<script>
const BASE = '';

async function fetchHealth() {
  try {
    const r = await fetch(`${BASE}/health`);
    const d = await r.json();
    document.getElementById('health').innerHTML =
      `<span class="text-green-600 font-medium">✔ Online</span> — ${d.service} v${d.version}`;
  } catch (e) {
    document.getElementById('health').textContent = '✖ Unreachable';
  }
}

function apiKey() { return document.getElementById('apikey').value || 'dev-key'; }

function badge(state) {
  const cls = {
    pending: 'bg-yellow-100 text-yellow-800',
    running: 'bg-blue-100 text-blue-800',
    completed: 'bg-green-100 text-green-800',
    failed: 'bg-red-100 text-red-800',
    cancelled: 'bg-gray-100 text-gray-800',
  }[state] || 'bg-gray-100 text-gray-500';
  return `<span class="inline-block px-2 py-0.5 rounded-full text-xs font-medium ${cls}">${state}</span>`;
}

async function loadPipelines() {
  const el = document.getElementById('pipeline-list');
  try {
    const r = await fetch(`${BASE}/pipelines`, {
      headers: { 'X-Api-Key': apiKey() }
    });
    if (!r.ok) { el.textContent = 'Unauthorized — check API key'; return; }
    const list = await r.json();
    if (!list.length) { el.textContent = 'No pipelines yet'; return; }
    el.innerHTML = list.map(p => `
      <div class="flex items-center justify-between border border-gray-200 rounded-lg px-4 py-3">
        <div>
          <span class="font-mono text-xs text-gray-500">${p.id.slice(0,8)}…</span>
          ${badge(p.state)}
          ${p.error ? `<span class="ml-2 text-red-500 text-xs">${p.error}</span>` : ''}
        </div>
        <span class="text-xs text-gray-400">${new Date(p.submitted_at * 1000).toLocaleString()}</span>
      </div>`).join('');
  } catch(e) {
    el.textContent = 'Error loading pipelines: ' + e.message;
  }
}

async function submitPipeline() {
  const el = document.getElementById('submit-result');
  const raw = document.getElementById('pipelineDef').value.trim();
  let definition;
  try { definition = JSON.parse(raw || '{}'); } catch(e) {
    el.textContent = '✖ Invalid JSON: ' + e.message; return;
  }
  try {
    const r = await fetch(`${BASE}/pipelines`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Api-Key': apiKey() },
      body: JSON.stringify({ definition }),
    });
    const d = await r.json();
    if (r.ok) {
      el.innerHTML = `<span class="text-green-600">✔ Submitted: <code>${d.id}</code></span>`;
      loadPipelines();
    } else {
      el.innerHTML = `<span class="text-red-600">✖ ${d.error || 'Unknown error'}</span>`;
    }
  } catch(e) {
    el.textContent = '✖ Network error: ' + e.message;
  }
}

fetchHealth();
loadPipelines();
setInterval(loadPipelines, 10_000);
</script>
</body>
</html>
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt; // for `oneshot`

    fn test_state() -> AppState {
        AppState::new("test-key")
    }

    async fn body_json(body: axum::body::Body) -> Value {
        let bytes = to_bytes(body, usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = build_router(test_state());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res.into_body()).await;
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn submit_pipeline_requires_api_key() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method(Method::POST)
            .uri("/pipelines")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"definition":{}}"#))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn submit_and_list_pipeline() {
        let app = build_router(test_state());
        // Submit
        let req = Request::builder()
            .method(Method::POST)
            .uri("/pipelines")
            .header("content-type", "application/json")
            .header("x-api-key", "test-key")
            .body(Body::from(r#"{"definition":{"nodes":[]}}"#))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_json(res.into_body()).await;
        let id = body["id"].as_str().unwrap().to_string();
        assert!(!id.is_empty());

        // List
        let req = Request::builder()
            .uri("/pipelines")
            .header("x-api-key", "test-key")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let list = body_json(res.into_body()).await;
        assert!(list.as_array().unwrap().iter().any(|p| p["id"] == id));
    }

    #[tokio::test]
    async fn delete_pipeline_removes_it() {
        let state = test_state();
        // Pre-insert a pipeline
        let id = Uuid::new_v4().to_string();
        state
            .pipelines
            .insert(id.clone(), PipelineRun::new(id.clone(), json!({})));

        let app = build_router(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/pipelines/{id}"))
            .header("x-api-key", "test-key")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn get_unknown_pipeline_returns_404() {
        let app = build_router(test_state());
        let req = Request::builder()
            .uri("/pipelines/does-not-exist")
            .header("x-api-key", "test-key")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dashboard_returns_html() {
        let app = build_router(test_state());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers()["content-type"].to_str().unwrap();
        assert!(ct.contains("text/html"));
    }
}
