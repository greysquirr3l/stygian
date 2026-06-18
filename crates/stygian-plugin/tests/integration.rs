//! Integration tests for the MCP request handler and protocol

#![cfg_attr(test, allow(clippy::panic))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn
)]

use serde_json::json;
use std::sync::Arc;
use stygian_plugin::{McpPluginServer, McpRequestHandler, config::Config};
use tempfile::TempDir;

fn test_handler() -> (TempDir, McpRequestHandler) {
    let tmp = match TempDir::new() {
        Ok(tmp) => tmp,
        Err(e) => panic!("failed to create temp dir for integration test: {e}"),
    };
    let server = Arc::new(McpPluginServer::new_with_file_storage(
        tmp.path().to_path_buf(),
    ));
    let config = Config::testing();
    let handler = McpRequestHandler::new(server, config);
    (tmp, handler)
}

#[tokio::test]
async fn test_initialize_handshake() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-11-25" }
    });

    let Some(resp) = handler.handle(&req).await else {
        panic!("Expected response from initialize");
    };

    // Check response structure
    assert_eq!(
        resp.get("jsonrpc").and_then(serde_json::Value::as_str),
        Some("2.0")
    );
    assert_eq!(resp.get("id").and_then(serde_json::Value::as_u64), Some(1));

    // Check protocol version negotiation
    let proto = resp
        .pointer("/result/protocolVersion")
        .and_then(serde_json::Value::as_str);
    assert_eq!(proto, Some("2025-11-25"));

    // Check server info
    let server_name = resp
        .pointer("/result/serverInfo/name")
        .and_then(serde_json::Value::as_str);
    assert_eq!(server_name, Some("stygian-plugin-test"));
}

#[tokio::test]
async fn test_initialize_unsupported_protocol() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "initialize",
        "params": { "protocolVersion": "1999-01-01" }
    });

    let Some(resp) = handler.handle(&req).await else {
        panic!("Expected response from initialize");
    };

    // Should return error
    assert_eq!(
        resp.pointer("/error/code")
            .and_then(serde_json::Value::as_i64),
        Some(-32602)
    );
    let msg = resp
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str);
    assert!(msg.is_some_and(|m| m.contains("Unsupported")));
}

#[tokio::test]
async fn test_tools_list() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": {}
    });

    let Some(resp) = handler.handle(&req).await else {
        panic!("Expected response from tools/list");
    };

    // Check response
    let Some(tools) = resp
        .pointer("/result/tools")
        .and_then(serde_json::Value::as_array)
    else {
        unreachable!("Expected tools array in response");
    };

    // Should have at least 8 tools
    assert!(tools.len() >= 8);

    // Check for expected tool names
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(serde_json::Value::as_str))
        .collect();

    assert!(tool_names.contains(&"plugin_create_template"));
    assert!(tool_names.contains(&"plugin_list_templates"));
    assert!(tool_names.contains(&"plugin_apply_template"));
    assert!(tool_names.contains(&"plugin_extract_batch"));
}

#[tokio::test]
async fn test_ping() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "ping",
        "params": {}
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Should return empty object
    assert_eq!(
        resp.pointer("/result")
            .and_then(serde_json::Value::as_object)
            .map(serde_json::Map::len),
        Some(0)
    );
}

#[tokio::test]
async fn test_notification_no_response() {
    let (_tmp, handler) = test_handler();

    // Notification has no id field
    let req = json!({
        "jsonrpc": "2.0",
        "method": "initialized"
    });

    let resp = handler.handle(&req).await;

    // Notifications should not produce responses
    assert!(resp.is_none());
}

#[tokio::test]
async fn test_invalid_jsonrpc_version() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "1.0",
        "id": 5,
        "method": "ping"
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Should return error
    assert_eq!(
        resp.pointer("/error/code")
            .and_then(serde_json::Value::as_i64),
        Some(-32600)
    );
}

#[tokio::test]
async fn test_missing_method() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "params": {}
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Should return error
    assert_eq!(
        resp.pointer("/error/code")
            .and_then(serde_json::Value::as_i64),
        Some(-32600)
    );
}

#[tokio::test]
async fn test_method_not_found() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "nonexistent/method"
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Should return -32601 (Method not found)
    assert_eq!(
        resp.pointer("/error/code")
            .and_then(serde_json::Value::as_i64),
        Some(-32601)
    );
}

#[tokio::test]
async fn test_tools_call_missing_name() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": { "arguments": {} }
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Should return error
    assert_eq!(
        resp.pointer("/error/code")
            .and_then(serde_json::Value::as_i64),
        Some(-32602)
    );
}

#[tokio::test]
async fn test_initialize_without_protocol_version() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "initialize",
        "params": {}
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Should return success with default protocol version
    assert_eq!(resp.get("error"), None);
    let proto = resp
        .pointer("/result/protocolVersion")
        .and_then(serde_json::Value::as_str);
    assert!(proto.is_some()); // Should have defaulted to first supported version
}

#[tokio::test]
async fn test_response_format_correctness() {
    let (_tmp, handler) = test_handler();

    let req = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/list",
        "params": {}
    });

    let resp = handler.handle(&req).await;
    assert!(resp.is_some(), "Expected response");
    let Some(resp) = resp else {
        unreachable!("Already asserted is_some");
    };

    // Verify JSON-RPC 2.0 response format
    assert_eq!(
        resp.get("jsonrpc").and_then(serde_json::Value::as_str),
        Some("2.0")
    );
    assert_eq!(resp.get("id").and_then(serde_json::Value::as_u64), Some(10));
    assert!(resp.get("result").is_some());
    assert_eq!(resp.get("error"), None);
}
