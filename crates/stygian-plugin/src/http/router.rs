//! Axum router for the HTTP MCP transport.
//!
//! Provides JSON-RPC 2.0 over HTTP with permissive CORS for Chrome extensions.

use super::state::AppState;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use serde_json::{Value, json};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, warn};

/// Build the full axum router with CORS middleware attached.
///
/// # CORS Policy
///
/// - Allows `chrome-extension://*`, `http://localhost:*`, and `http://127.0.0.1:*`
/// - Allows `Content-Type`, `Authorization`, and `X-Request-ID` headers
/// - Allows `GET`, `POST`, and `OPTIONS`
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any) // Chrome extensions use unique opaque origins
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::HeaderName::from_static("x-request-id"),
        ]);

    Router::new()
        .route("/health", get(health_handler))
        .route("/mcp", post(mcp_dispatch_handler))
        .route("/mcp/tools/list", get(tools_list_handler))
        .route("/mcp/tools/call", post(tools_call_handler))
        .layer(cors)
        .with_state(state)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /health` — liveness probe.
///
/// Returns 200 OK with `{"status":"ok","service":"stygian-plugin-mcp"}`.
async fn health_handler() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "stygian-plugin-mcp"
    }))
}

/// `POST /mcp` — full JSON-RPC 2.0 dispatch.
///
/// Accepts any well-formed JSON-RPC 2.0 request body and routes it through
/// the [`McpRequestHandler`] exactly as the stdio transport would.
///
/// # Notifications
///
/// JSON-RPC notifications (requests without an `id` field) return `204 No Content`.
async fn mcp_dispatch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let req_id = extract_request_id(&headers);
    debug!(
        req_id,
        method = body.get("method").and_then(|v| v.as_str()),
        "MCP dispatch"
    );

    // Notification returns None per JSON-RPC spec — no response body
    state
        .handler
        .handle(&body)
        .await
        .map_or((StatusCode::NO_CONTENT, Json(Value::Null)), |response| {
            (StatusCode::OK, Json(response))
        })
}

/// `GET /mcp/tools/list` — enumerate available tools.
///
/// Convenience endpoint that wraps a `tools/list` request; useful for health
/// checks, browser extension discovery, and debugging.
async fn tools_list_handler(State(state): State<AppState>) -> Json<Value> {
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    state.handler.handle(&req).await.map_or_else(
        || Json(json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [] } })),
        Json,
    )
}

/// `POST /mcp/tools/call` — invoke a single MCP tool.
///
/// Accepts the Chrome extension's preferred envelope:
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "id": 42,
///   "method": "tools/call",
///   "params": { "name": "plugin_apply_template", "arguments": { ... } }
/// }
/// ```
///
/// The outer JSON-RPC envelope is optional — bare `{"name": ..., "arguments": ...}`
/// is also accepted and will be wrapped automatically.
async fn tools_call_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let req_id = extract_request_id(&headers);

    // Normalize: if caller sent bare args (no jsonrpc key), wrap them
    let request = if body.get("jsonrpc").is_some() {
        body
    } else {
        // Bare call: {"name": "...", "arguments": {...}}
        debug!(req_id, "bare tool call — wrapping in JSON-RPC envelope");
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": body
        })
    };

    // Validate the tool name is present before dispatching
    let tool_name = request
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str);

    if tool_name.is_none() {
        warn!(req_id, "tool call missing 'params.name'");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "jsonrpc": "2.0",
                "id": request.get("id").cloned().unwrap_or(Value::Null),
                "error": {
                    "code": -32602,
                    "message": "Invalid params: missing 'name' in params"
                }
            })),
        );
    }

    debug!(req_id, tool = tool_name, "tool call");

    state
        .handler
        .handle(&request)
        .await
        .map_or((StatusCode::NO_CONTENT, Json(Value::Null)), |response| {
            (StatusCode::OK, Json(response))
        })
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the `X-Request-ID` header value for structured logging, or return `"-"`.
fn extract_request_id(headers: &HeaderMap) -> &str {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
}
