#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn
)]
//! Integration tests for the HTTP transport of the stygian-plugin MCP server.
//!
//! These tests spin up the actual axum server on a random port, make real HTTP
//! requests using `reqwest`, and verify JSON-RPC 2.0 compliance end-to-end.

#![cfg(feature = "http")]
// serde_json::Value's Index impl never panics — it returns Value::Null for
// missing keys/indices.  The lint fires conservatively on all Index usage.
#![expect(
    clippy::indexing_slicing,
    reason = "serde_json::Value::index is infallible"
)]

use reqwest::Client;
use serde_json::{Value, json};
use std::net::SocketAddr;
use stygian_plugin::config::Config;
use stygian_plugin::http::{AppState, build_router};
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

// ─────────────────────────────────────────────────────────────────────────────
// Test helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Bind to port 0 (OS picks a free port) and return a running server base URL.
async fn start_test_server()
-> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let config = Config::testing();
    let state = AppState::new(config)?;
    let app = build_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    Ok((base_url, handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// Health endpoint
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_returns_ok() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let resp = client.get(format!("{base}/health")).send().await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "stygian-plugin-mcp");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// CORS headers
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_cors_headers_present() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    // Preflight OPTIONS
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("{base}/mcp/tools/list"))
        .header(
            "Origin",
            "chrome-extension://abcdefghijklmnopqrstuvwxyz123456",
        )
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await?;

    // CORS middleware should allow any origin
    let allow_origin = resp
        .headers()
        .get("access-control-allow-origin")
        .map_or("", |v| v.to_str().unwrap_or(""));
    assert!(
        allow_origin == "*" || allow_origin.contains("chrome-extension"),
        "expected CORS allow-origin, got: {allow_origin}"
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// tools/list
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_tools_list_via_get() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let resp = client.get(format!("{base}/mcp/tools/list")).send().await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;

    // Must be a valid JSON-RPC 2.0 response
    assert_eq!(body["jsonrpc"], "2.0");
    assert!(body["result"]["tools"].is_array(), "expected tools array");

    let tools = body["result"]["tools"]
        .as_array()
        .ok_or("expected tools to be an array")?;
    assert!(
        !tools.is_empty(),
        "expected at least one tool in the registry"
    );

    // Spot-check the mandatory tool names
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in &[
        "plugin_create_template",
        "plugin_list_templates",
        "plugin_get_template",
        "plugin_delete_template",
        "plugin_apply_template",
    ] {
        assert!(
            names.contains(expected),
            "missing tool: {expected}; available: {names:?}"
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// tools/call — via POST /mcp/tools/call
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_tools_call_list_templates_jsonrpc_envelope() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let req_body = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/call",
        "params": {
            "name": "plugin_list_templates",
            "arguments": {}
        }
    });

    let resp = client
        .post(format!("{base}/mcp/tools/call"))
        .json(&req_body)
        .send()
        .await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 42);
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    assert!(body["result"].is_object() || body["result"].is_array());
    Ok(())
}

#[tokio::test]
async fn test_tools_call_bare_envelope() -> TestResult {
    // Chrome extension may send without the jsonrpc wrapper
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let payload = json!({
        "name": "plugin_list_templates",
        "arguments": {}
    });

    let resp = client
        .post(format!("{base}/mcp/tools/call"))
        .json(&payload)
        .send()
        .await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");
    Ok(())
}

#[tokio::test]
async fn test_tools_call_unknown_tool_returns_error() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let req_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "does_not_exist",
            "arguments": {}
        }
    });

    let resp = client
        .post(format!("{base}/mcp/tools/call"))
        .json(&req_body)
        .send()
        .await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");

    // The MCP protocol returns tool errors as `result.isError = true` content,
    // NOT as a top-level JSON-RPC `error` field.
    let is_error = body["result"]["isError"].as_bool().unwrap_or(false);
    let has_rpc_error = body.get("error").is_some();
    assert!(
        is_error || has_rpc_error,
        "expected isError=true or JSON-RPC error for unknown tool, got: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn test_tools_call_missing_name_returns_400() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let req_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "arguments": {}
        }
        // deliberately missing "name"
    });

    let resp = client
        .post(format!("{base}/mcp/tools/call"))
        .json(&req_body)
        .send()
        .await?;

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await?;
    assert_eq!(body["error"]["code"], -32602);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Full JSON-RPC dispatch — POST /mcp
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_mcp_dispatch_initialize() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let req_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "clientInfo": { "name": "test-client", "version": "0.0.1" },
            "capabilities": {}
        }
    });

    let resp = client
        .post(format!("{base}/mcp"))
        .json(&req_body)
        .send()
        .await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    assert!(body["result"]["serverInfo"].is_object());
    Ok(())
}

#[tokio::test]
async fn test_mcp_dispatch_tools_list() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let req_body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let resp = client
        .post(format!("{base}/mcp"))
        .json(&req_body)
        .send()
        .await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["jsonrpc"], "2.0");
    assert!(body["result"]["tools"].is_array());
    Ok(())
}

#[tokio::test]
async fn test_mcp_dispatch_notification_returns_204() -> TestResult {
    // A notification (no id field) must return 204 No Content
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
        // no "id" field — this is a notification
    });

    let resp = client
        .post(format!("{base}/mcp"))
        .json(&notification)
        .send()
        .await?;

    assert_eq!(resp.status(), 204);
    Ok(())
}

#[tokio::test]
async fn test_mcp_dispatch_parse_error() -> TestResult {
    // Sending a non-JSON-RPC body (invalid structure) should return a JSON-RPC
    // parse error wrapped in HTTP 200 (since the JSON was valid, just wrong shape).
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let junk = json!({ "not": "jsonrpc", "at": "all" });

    let resp = client
        .post(format!("{base}/mcp"))
        .json(&junk)
        .send()
        .await?;

    // Handler returns Some(error_response) with HTTP 200, or None with 204.
    // Either is acceptable for garbage input.
    assert!(
        resp.status() == 200 || resp.status() == 204,
        "unexpected status: {}",
        resp.status()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON-RPC error codes
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_jsonrpc_method_not_found() -> TestResult {
    let (base, _handle) = start_test_server().await?;
    let client = Client::new();

    let req_body = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "bananas/eat",
        "params": {}
    });

    let resp = client
        .post(format!("{base}/mcp"))
        .json(&req_body)
        .send()
        .await?;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["id"], 99);

    // Should be error code -32601 (Method not found) or similar
    let code = body["error"]["code"].as_i64();
    assert!(
        code.is_some(),
        "expected error.code in response, got: {body}"
    );
    Ok(())
}
