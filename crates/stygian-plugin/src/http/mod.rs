//! HTTP transport for the stygian-plugin MCP server.
//!
//! Exposes the full MCP JSON-RPC 2.0 API over HTTP with CORS support, enabling
//! the Chrome browser extension to communicate with the local server directly.
//!
//! # Endpoints
//!
//! | Method | Path                  | Purpose                                      |
//! |--------|-----------------------|----------------------------------------------|
//! | GET    | `/health`             | Liveness probe — returns `{"status":"ok"}`   |
//! | GET    | `/mcp/tools/list`     | Enumerate available MCP tools                |
//! | POST   | `/mcp/tools/call`     | Invoke a tool with JSON-RPC 2.0 envelope     |
//! | POST   | `/mcp`                | Full JSON-RPC 2.0 dispatch (all methods)     |
//!
//! # CORS
//!
//! All endpoints permit `chrome-extension://` and `http://localhost` origins so
//! the browser extension can reach the server without a proxy.
//!
//! # Example
//!
//! ```no_run
//! use stygian_plugin::http::HttpServer;
//! use stygian_plugin::config::Config;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let config = Config::testing();
//!     let server = HttpServer::new(config)?;
//!     server.run().await
//! }
//! ```

mod router;
mod state;

pub use router::build_router;
pub use state::AppState;

use crate::config::Config;
use crate::error::PluginError;
use crate::error::Result as PluginResult;
use std::net::SocketAddr;
use tracing::info;

/// HTTP server wrapping the MCP handler over axum.
pub struct HttpServer {
    config: Config,
}

impl HttpServer {
    /// Create a new HTTP server from config.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] if the state cannot be initialized.
    pub const fn new(config: Config) -> PluginResult<Self> {
        Ok(Self { config })
    }

    /// Start the HTTP server and block until shutdown.
    ///
    /// Binds to `0.0.0.0:<port>` (default 3000, overridden by `--http-port`).
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] on bind failure or server error.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let port = self.config.http_port;
        let state = AppState::new(self.config)?;
        let app = build_router(state);
        let addr = SocketAddr::from(([0, 0, 0, 0], port));

        info!(addr = %addr, "HTTP MCP server listening");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app)
            .await
            .map_err(|e| PluginError::Other(format!("HTTP server error: {e}")))?;
        Ok(())
    }
}
