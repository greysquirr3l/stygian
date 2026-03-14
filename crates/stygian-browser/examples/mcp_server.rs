//! MCP browser server example.
//!
//! Runs a Model Context Protocol server that exposes browser pool operations
//! over stdin/stdout JSON-RPC 2.0. Connect any MCP-compatible client (VS Code
//! Copilot, Claude Desktop, etc.) to control a real Chrome browser.
//!
//! # Usage
//!
//! ```sh
//! STYGIAN_MCP_ENABLED=true \
//!   cargo run --example mcp_server -p stygian-browser --features mcp
//! ```
//!
//! # Environment variables
//!
//! | Variable | Default | Description |
//! | ---------- | --------- | ------------- |
//! | `STYGIAN_MCP_ENABLED` | `false` | Must be `true` to start the server |
//! | `RUST_LOG` | `info` | Tracing filter (use `debug` for CDP traffic) |

#[cfg(feature = "mcp")]
use stygian_browser::mcp::{McpBrowserServer, is_mcp_enabled};
#[cfg(feature = "mcp")]
use stygian_browser::{BrowserConfig, BrowserPool};

#[cfg(not(feature = "mcp"))]
fn main() {
    eprintln!("This example requires the `mcp` feature.");
    eprintln!("Run with: cargo run --example mcp_server -p stygian-browser --features mcp");
    std::process::exit(1);
}

#[cfg(feature = "mcp")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialise structured logging to stderr (MCP uses stdout for protocol)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if !is_mcp_enabled() {
        eprintln!("MCP server is disabled. Set STYGIAN_MCP_ENABLED=true to enable.");
        std::process::exit(1);
    }

    tracing::info!("Starting stygian-browser MCP server");

    // Build browser pool
    let pool = BrowserPool::new(BrowserConfig::default()).await?;

    // Run the MCP server
    let server = McpBrowserServer::new(pool);
    server.run().await?;

    Ok(())
}
