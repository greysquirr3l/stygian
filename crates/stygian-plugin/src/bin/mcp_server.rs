#!/usr/bin/env -S cargo run --bin stygian-plugin-mcp --
#![allow(clippy::panic)]
//! Standalone MCP server for stygian-plugin
//!
//! Implements full JSON-RPC 2.0 protocol over stdin/stdout.
//!
//! # Usage
//!
//! ```sh
//! stygian-plugin-mcp --templates-dir ./plugin-templates --log-level info
//! ```
//!
//! # Protocol
//!
//! - Input: newline-delimited JSON (one request per line)
//! - Output: newline-delimited JSON (one response per line)
//! - Notifications (id=null): no response sent
//! - Errors: JSON-RPC 2.0 error codes (-32600 to -32700)

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info};

use stygian_plugin::{McpPluginServer, McpRequestHandler, config::Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Parse configuration from CLI args and environment
    let config = Config::from_args();

    // Initialize logging
    init_logging(&config.log_level);
    info!("stygian-plugin MCP server starting");
    info!(
        templates_dir = ?config.templates_dir,
        server = config.server_name,
        version = env!("CARGO_PKG_VERSION"),
        "configuration loaded"
    );

    // Create the server
    let server = Arc::new(McpPluginServer::new_with_file_storage(
        config.templates_dir.clone(),
    ));
    let handler = McpRequestHandler::new(Arc::clone(&server), config);

    // Run the JSON-RPC transport loop
    run_mcp_server(handler).await
}

/// Run the MCP server over stdin/stdout
async fn run_mcp_server(
    handler: McpRequestHandler,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("MCP server listening on stdin/stdout (JSON-RPC 2.0)");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = stdout;

    while let Some(line) = reader.next_line().await? {
        let line = line.trim().to_string();

        // Skip empty lines
        if line.is_empty() {
            continue;
        }

        debug!(?line, "incoming request");

        // Parse JSON and dispatch
        let response = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(req) => handler.handle(&req).await,
            Err(e) => {
                debug!(error = %e, "parse error");
                Some(make_parse_error(&e))
            }
        };

        // Send response (notifications return None and don't get a response)
        if let Some(mut resp) = response {
            // Ensure response has correct format
            if !resp.is_object() {
                resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": serde_json::Value::Null,
                    "error": {
                        "code": -32603,
                        "message": "Internal error: invalid response format"
                    }
                });
            }

            let mut out = match serde_json::to_string(&resp) {
                Ok(s) => s,
                Err(e) => {
                    error!(error = %e, "failed to serialize response");
                    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Serialization error"}}"#
                        .to_string()
                }
            };
            out.push('\n');

            if let Err(e) = stdout.write_all(out.as_bytes()).await {
                error!(error = %e, "failed to write response");
                break;
            }
            if let Err(e) = stdout.flush().await {
                error!(error = %e, "failed to flush stdout");
                break;
            }
        }
    }

    info!("MCP server stopping (stdin closed)");
    Ok(())
}

/// Create a JSON-RPC parse error response
fn make_parse_error(e: &serde_json::error::Error) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "error": {
            "code": -32700,
            "message": format!("Parse error: {e}")
        }
    })
}

/// Initialize the logging system
fn init_logging(level: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_parse_error() {
        let json_result = serde_json::from_str::<serde_json::Value>("invalid json");
        assert!(
            json_result.is_err(),
            "Expected parse error for invalid json"
        );
        let Err(json_err) = json_result else {
            unreachable!("Already asserted is_err");
        };
        let err_resp = make_parse_error(&json_err);

        assert_eq!(
            err_resp
                .pointer("/jsonrpc")
                .and_then(serde_json::Value::as_str),
            Some("2.0")
        );
        assert_eq!(
            err_resp
                .pointer("/error/code")
                .and_then(serde_json::Value::as_i64),
            Some(-32700)
        );
        assert!(
            err_resp
                .pointer("/error/message")
                .is_some_and(|s| s.as_str().is_some_and(|s| s.contains("Parse error")))
        );
    }
}
