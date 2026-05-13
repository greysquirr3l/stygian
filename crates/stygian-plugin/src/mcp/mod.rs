//! MCP (Model Context Protocol) server for the plugin extraction system
//!
//! Exposes plugin-based visual data extraction as an MCP server with full
//! protocol support including initialization, tool discovery, and execution.
//!
//! # Standalone Server
//!
//! Run as a standalone server over stdin/stdout:
//!
//! ```sh
//! cargo run --bin stygian-plugin-mcp -- --templates-dir ./templates
//! ```
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │ Standalone MCP Server Binary      │
//! │ (stdio JSON-RPC 2.0 transport)    │
//! └─────────────┬──────────────────────┘
//!               │
//! ┌─────────────▼──────────────────────┐
//! │ McpRequestHandler                  │
//! │ (initialize, tools/list,call)      │
//! └─────────────┬──────────────────────┘
//! │
//! ┌─────────────▼──────────────────────┐
//! │ McpPluginServer                    │
//! │ (8 extraction & template tools)    │
//! └──────────────────────────────────┘
//! ```

pub mod handler;
pub mod server;

pub use handler::McpRequestHandler;
pub use server::McpPluginServer;
