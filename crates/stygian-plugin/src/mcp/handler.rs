//! MCP request dispatcher and protocol handler.
//!
//! Implements the full MCP protocol including initialize, notifications,
//! and tool dispatch.

use crate::config::{Config, SUPPORTED_PROTOCOL_VERSIONS};
use crate::mcp::McpPluginServer;
use serde_json::{Value, json};
use std::sync::Arc;

/// Dispatches incoming JSON-RPC 2.0 MCP requests to appropriate handlers.
pub struct McpRequestHandler {
    server: Arc<McpPluginServer>,
    config: Config,
}

impl McpRequestHandler {
    /// Create a new request handler
    pub const fn new(server: Arc<McpPluginServer>, config: Config) -> Self {
        Self { server, config }
    }

    /// Handle an incoming MCP request.
    ///
    /// Returns `Some(response)` for all requests except notifications (id field missing),
    /// which return `None`.
    pub async fn handle(&self, req: &Value) -> Option<Value> {
        // Check if this is a well-formed notification (jsonrpc="2.0", has method, id field missing)
        let is_notification = is_jsonrpc_notification(req);
        let id = req.get("id").unwrap_or(&Value::Null);

        // Validate JSON-RPC 2.0 structure
        if req.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                id,
                -32600,
                "Invalid request: expected jsonrpc='2.0'",
            ));
        }

        let Some(method) = req.get("method").and_then(Value::as_str) else {
            return Some(error_response(
                id,
                -32600,
                "Invalid request: missing string 'method'",
            ));
        };

        // Dispatch to appropriate handler
        let response = match method {
            "initialize" => self.handle_initialize(id, req),
            "initialized" | "notifications/initialized" | "ping" => ok_response(id, &json!({})),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req).await,
            other => error_response(id, -32601, &format!("Method not found: {other}")),
        };

        // Notifications don't get responses
        if is_notification {
            None
        } else {
            Some(response)
        }
    }

    /// Handle the initialize request
    fn handle_initialize(&self, id: &Value, req: &Value) -> Value {
        let requested_version = req
            .get("params")
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str);

        let protocol_version = match requested_version {
            Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
            Some(v) => {
                return error_response(
                    id,
                    -32602,
                    &format!(
                        "Unsupported protocolVersion: {v}. Supported: {}",
                        SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                    ),
                );
            }
            None => SUPPORTED_PROTOCOL_VERSIONS
                .first()
                .copied()
                .unwrap_or("2024-11-05"),
        };

        ok_response(
            id,
            &json!({
                "protocolVersion": protocol_version,
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": self.config.server_name,
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    /// Handle tools/list request
    fn handle_tools_list(&self, id: &Value) -> Value {
        let tools = self.server.tools_list();
        ok_response(id, &json!({ "tools": tools }))
    }

    /// Handle tools/call request
    async fn handle_tools_call(&self, id: &Value, req: &Value) -> Value {
        let Some(params) = req.get("params") else {
            return error_response(id, -32602, "Missing 'params'");
        };

        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing tool 'name'");
        };

        let empty = Value::Null;
        let args = params.get("arguments").unwrap_or(&empty);

        // Call the server's tool handler
        let result = self.server.handle_tool_call(name, args).await;

        // Return wrapped in MCP response format
        ok_response(id, &result)
    }
}

// ─── Response Helpers ───────────────────────────────────────────────────────

/// Construct a successful JSON-RPC response
fn ok_response(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Construct an error JSON-RPC response
fn error_response(id: &Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// Check if a request is a valid JSON-RPC notification (no response required)
fn is_jsonrpc_notification(req: &Value) -> bool {
    req.is_object()
        && req.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && req.get("id").is_none()
        && req.get("method").and_then(Value::as_str).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_jsonrpc_notification() {
        // Valid notification: jsonrpc 2.0, method, no id
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "ping"
        });
        assert!(is_jsonrpc_notification(&notif));

        // Not a notification: has id
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping"
        });
        assert!(!is_jsonrpc_notification(&request));

        // Not a notification: missing jsonrpc
        let bad = json!({ "method": "ping" });
        assert!(!is_jsonrpc_notification(&bad));
    }

    #[test]
    fn test_ok_response() {
        let resp = ok_response(&json!(1), &json!({"status": "ok"}));
        assert_eq!(resp.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(resp.get("id").and_then(Value::as_u64), Some(1));
        assert_eq!(
            resp.pointer("/result/status").and_then(Value::as_str),
            Some("ok")
        );
    }

    #[test]
    fn test_error_response() {
        let resp = error_response(&json!(2), -32601, "Not found");
        assert_eq!(resp.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(resp.get("id").and_then(Value::as_u64), Some(2));
        assert_eq!(
            resp.pointer("/error/code").and_then(Value::as_i64),
            Some(-32601)
        );
        assert_eq!(
            resp.pointer("/error/message").and_then(Value::as_str),
            Some("Not found")
        );
    }
}
