#![allow(clippy::multiple_crate_versions)]
//! `stygian-mcp` — unified MCP server for stygian.
//!
//! Starts the aggregated MCP server that merges stygian-graph,
//! stygian-browser, and stygian-proxy capabilities into a single
//! JSON-RPC 2.0 stdin/stdout server.
//!
//! # Usage
//!
//! ```sh
//! cargo run --bin stygian-mcp
//! ```
//!
//! Configure VS Code (or any MCP client) to launch this binary and pipe
//! stdin/stdout.  The server advertises all tools from all three sub-crates
//! under the namespace conventions described in [`stygian_mcp::aggregator`].

use tracing_subscriber::EnvFilter;

use stygian_mcp::aggregator::McpAggregator;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise logging.  Writes to stderr so it does not pollute the
    // stdout MCP channel.  Control verbosity via `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let aggregator = McpAggregator::try_new()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start stygian-mcp aggregator: {e}"))?;

    aggregator
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("stygian-mcp aggregator exited with error: {e}"))
}
