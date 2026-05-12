//! Idempotency key type for safe extraction retries

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// An idempotency key for deduplicating extraction operations
///
/// Uses ULID format (chronological, sortable, unique).
/// Enables safe retries: if the same extraction is run twice with the same key,
/// the cached result is returned instead of re-executing.
///
/// # Example
///
/// ```
/// use stygian_plugin::domain::IdempotencyKey;
///
/// let key1 = IdempotencyKey::new();
/// let key2 = IdempotencyKey::new();
/// assert_ne!(key1, key2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct IdempotencyKey(ulid::Ulid);

impl IdempotencyKey {
    /// Generate a new idempotency key
    pub fn new() -> Self {
        Self(ulid::Ulid::new())
    }

    /// Create from an existing ULID
    pub const fn from_ulid(ulid: ulid::Ulid) -> Self {
        Self(ulid)
    }

    /// Get the inner ULID
    pub const fn inner(&self) -> ulid::Ulid {
        self.0
    }

    /// Get timestamp when this key was generated
    pub const fn timestamp(&self) -> u64 {
        self.0.timestamp_ms()
    }
}

impl FromStr for IdempotencyKey {
    type Err = crate::error::PluginError;

    fn from_str(s: &str) -> crate::Result<Self> {
        ulid::Ulid::from_str(s)
            .map(Self)
            .map_err(|e| crate::error::PluginError::Other(format!("Invalid ULID: {e}")))
    }
}

impl Default for IdempotencyKey {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_key_is_unique() {
        let key1 = IdempotencyKey::new();
        let key2 = IdempotencyKey::new();
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_key_serialization() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let key = IdempotencyKey::new();
        let json = serde_json::to_string(&key)?;
        let key2: IdempotencyKey = serde_json::from_str(&json)?;
        assert_eq!(key, key2);
        Ok(())
    }

    #[test]
    fn test_key_display() {
        let key = IdempotencyKey::new();
        let s = format!("{key}");
        assert!(!s.is_empty());
    }

    #[test]
    fn test_key_timestamp() {
        let key = IdempotencyKey::new();
        let ts = key.timestamp();
        assert!(ts > 0);
    }
}
