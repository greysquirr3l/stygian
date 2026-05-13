//! Standalone MCP server for stygian-plugin
//!
//! Implements full JSON-RPC 2.0 protocol over **stdin/stdout** (default) or **HTTP**.
//!
//! # Stdio mode (default — for LLM tool integrations)
//!
//! ```sh
//! stygian-plugin-mcp --templates-dir ./plugin-templates --log-level info
//! ```
//!
//! # HTTP mode (for Chrome extension and browser clients)
//!
//! ```sh
//! stygian-plugin-mcp --transport http --http-port 3000 --templates-dir ./plugin-templates
//! ```
//!
//! ## HTTP endpoints
//!
//! | Method | Path                  | Purpose                                   |
//! |--------|-----------------------|-------------------------------------------|
//! | GET    | `/health`             | Liveness probe                            |
//! | GET    | `/mcp/tools/list`     | List available tools                      |
//! | POST   | `/mcp/tools/call`     | Call a tool via JSON-RPC 2.0              |
//! | POST   | `/mcp`                | Full JSON-RPC 2.0 dispatch                |
//!
//! # Protocol
//!
//! - **Stdio**: newline-delimited JSON (one request per line, one response per line)
//! - **HTTP**: JSON-RPC 2.0 over HTTP with permissive CORS for Chrome extensions
//! - Notifications (id absent): no response in either mode (204 in HTTP mode)
//! - Errors: standard JSON-RPC 2.0 error codes (-32600 to -32700)

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info};

use stygian_plugin::config::{Config, TransportMode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_args();
    init_logging(&config.log_level);

    info!("stygian-plugin MCP server starting");
    info!(
        templates_dir = ?config.templates_dir,
        server = config.server_name,
        transport = ?config.transport,
        version = env!("CARGO_PKG_VERSION"),
        "configuration loaded"
    );

    match config.transport {
        TransportMode::Stdio => {
            let server = Arc::new(
                stygian_plugin::mcp::server::McpPluginServer::new_with_file_storage(
                    config.templates_dir.clone(),
                ),
            );
            let handler =
                stygian_plugin::mcp::handler::McpRequestHandler::new(Arc::clone(&server), config);
            run_stdio(handler).await
        }
        TransportMode::Http => {
            #[cfg(feature = "http")]
            {
                let http_server = stygian_plugin::http::HttpServer::new(config)
                    .map_err(|e| format!("failed to build HTTP server: {e}"))?;
                http_server.run().await
            }
            #[cfg(not(feature = "http"))]
            {
                eprintln!(
                    "error: HTTP transport requires the `http` feature.\n\
                     Rebuild with: cargo install --path crates/stygian-plugin --features http"
                );
                std::process::exit(1);
            }
        }
    }
}

/// Run the MCP server over stdin/stdout (JSON-RPC 2.0 newline-delimited).
async fn run_stdio(
    handler: stygian_plugin::mcp::handler::McpRequestHandler,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("MCP server listening on stdin/stdout (JSON-RPC 2.0)");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = stdout;

    while let Some(line) = reader.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        debug!(?line, "incoming request");

        let response = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(req) => handler.handle(&req).await,
            Err(e) => {
                debug!(error = %e, "parse error");
                Some(make_parse_error(&e))
            }
        };

        if let Some(mut resp) = response {
            if !resp.is_object() {
                resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": serde_json::Value::Null,
                    "error": { "code": -32603, "message": "Internal error: invalid response format" }
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

/// Build a JSON-RPC 2.0 parse error response.
fn make_parse_error(e: &serde_json::error::Error) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "error": { "code": -32700, "message": format!("Parse error: {e}") }
    })
}

/// Initialize the tracing subscriber from the requested level string.
fn init_logging(level: &str) {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(false)
        .compact()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_parse_error() {
        let parse_result = serde_json::from_str::<serde_json::Value>("invalid json");
        assert!(
            parse_result.is_err(),
            "serde_json must fail on invalid input"
        );
        let Err(json_err) = parse_result else {
            return;
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
                .is_some_and(|v| v.as_str().is_some_and(|s| s.contains("Parse error")))
        );
    }
}
