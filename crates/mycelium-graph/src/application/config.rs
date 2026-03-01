//! Configuration management

use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Log level
    pub log_level: String,
    /// Worker pool size
    pub workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            workers: 4,
        }
    }
}
