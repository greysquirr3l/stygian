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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_log_level_is_info() {
        let cfg = Config::default();
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn default_workers_is_four() {
        let cfg = Config::default();
        assert_eq!(cfg.workers, 4);
    }

    #[test]
    fn config_serializes_to_json() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("log_level"));
        assert!(json.contains("workers"));
    }

    #[test]
    fn config_roundtrips_through_json() {
        let cfg = Config {
            log_level: "debug".to_string(),
            workers: 8,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.log_level, "debug");
        assert_eq!(back.workers, 8);
    }
}
