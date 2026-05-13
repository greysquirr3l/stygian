//! Runtime configuration for the standalone MCP server.

use clap::Parser;
use std::path::PathBuf;

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
    }
}
