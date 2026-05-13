//! Runtime configuration for the standalone MCP server.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Transport mode for the MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum TransportMode {
    /// JSON-RPC 2.0 over stdin/stdout (default; for LLM tool integrations).
    #[default]
    Stdio,
    /// JSON-RPC 2.0 over HTTP (for Chrome extension and browser clients).
    Http,
}

/// Command-line configuration for stygian-plugin MCP server.
#[derive(Debug, Clone, Parser)]
#[command(name = "stygian-plugin-mcp")]
#[command(about = "Standalone MCP server for stygian-plugin extraction", long_about = None)]
pub struct Config {
    /// Directory where extraction templates are stored (JSON files)
    #[arg(long, value_name = "PATH", default_value = "./plugin-templates")]
    pub templates_dir: PathBuf,

    /// Logging level (off, error, warn, info, debug, trace)
    #[arg(long, value_name = "LEVEL", default_value = "info")]
    pub log_level: String,

    /// Server name advertised in MCP initialize response
    #[arg(long, value_name = "NAME", default_value = "stygian-plugin")]
    pub server_name: String,

    /// Transport mode: stdio (default) or http
    ///
    /// Use `http` to expose JSON-RPC 2.0 over HTTP so the Chrome browser
    /// extension can connect directly via `http://localhost:<port>/mcp/tools/call`.
    #[arg(long, value_name = "MODE", default_value = "stdio")]
    pub transport: TransportMode,

    /// Port for the HTTP transport (only used when --transport=http)
    #[arg(long, value_name = "PORT", default_value = "3000")]
    pub http_port: u16,
}

impl Config {
    /// Parse configuration from command-line arguments and environment variables.
    pub fn from_args() -> Self {
        Self::parse()
    }

    /// Create a test configuration with sensible defaults.
    pub fn testing() -> Self {
        Self {
            templates_dir: PathBuf::from("./test-templates"),
            log_level: "debug".to_string(),
            server_name: "stygian-plugin-test".to_string(),
            transport: TransportMode::Stdio,
            http_port: 3000,
        }
    }

    /// Create a test configuration pointing to an HTTP transport on a given port.
    pub fn testing_http(port: u16) -> Self {
        Self {
            templates_dir: PathBuf::from("./test-templates"),
            log_level: "debug".to_string(),
            server_name: "stygian-plugin-test".to_string(),
            transport: TransportMode::Http,
            http_port: port,
        }
    }
}

/// Supported MCP protocol versions, in preferred order.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2024-11-05"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let cfg = Config::testing();
        assert_eq!(cfg.server_name, "stygian-plugin-test");
        assert_eq!(cfg.log_level, "debug");
        assert_eq!(cfg.transport, TransportMode::Stdio);
        assert_eq!(cfg.http_port, 3000);
    }

    #[test]
    fn test_config_http() {
        let cfg = Config::testing_http(8080);
        assert_eq!(cfg.transport, TransportMode::Http);
        assert_eq!(cfg.http_port, 8080);
    }
}
