//! In-memory idempotency key store for tracking extraction results

use crate::Result;
use crate::domain::{ExtractionResult, IdempotencyKey};
use crate::ports::IdempotencyKeyStore;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory idempotency store
///
/// Tracks extraction results by idempotency key to prevent duplicate processing.
/// Useful for development and testing; production should use persistent storage.
///
/// # Example
///
/// ```
/// use stygian_plugin::storage::MemoryIdempotencyStore;
/// use stygian_plugin::domain::{IdempotencyKey, ExtractionResult};
///
/// let store = MemoryIdempotencyStore::new();
/// ```
pub struct MemoryIdempotencyStore {
    results: Arc<RwLock<HashMap<IdempotencyKey, ExtractionResult>>>,
}

impl MemoryIdempotencyStore {
    /// Create a new memory-based idempotency store
    pub fn new() -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for MemoryIdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IdempotencyKeyStore for MemoryIdempotencyStore {
    async fn store_result(&self, key: &IdempotencyKey, result: &ExtractionResult) -> Result<()> {
        self.results.write().await.insert(*key, result.clone());
        Ok(())
    }

    async fn get_result(&self, key: &IdempotencyKey) -> Result<Option<ExtractionResult>> {
        let results = self.results.read().await;
        Ok(results.get(key).cloned())
    }

    async fn delete_result(&self, key: &IdempotencyKey) -> Result<()> {
        self.results.write().await.remove(key);
        Ok(())
    }

    async fn clear_all(&self) -> Result<()> {
        self.results.write().await.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_get_result() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryIdempotencyStore::new();
        let key = IdempotencyKey::new();
        let result = ExtractionResult::new(key);

        store.store_result(&key, &result).await?;
        let retrieved = store.get_result(&key).await?;

        assert!(retrieved.is_some());
        let retrieved = retrieved.ok_or("expected Some result")?;
        assert_eq!(retrieved.metadata.idempotency_key, key);
        Ok(())
    }

    #[tokio::test]
    async fn test_get_nonexistent_result() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryIdempotencyStore::new();
        let key = IdempotencyKey::new();

        let result = store.get_result(&key).await?;
        assert!(result.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_result() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryIdempotencyStore::new();
        let key = IdempotencyKey::new();
        let result = ExtractionResult::new(key);

        store.store_result(&key, &result).await?;
        store.delete_result(&key).await?;

        let retrieved = store.get_result(&key).await?;
        assert!(retrieved.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_clear_all() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryIdempotencyStore::new();

        let key1 = IdempotencyKey::new();
        let key2 = IdempotencyKey::new();

        store
            .store_result(&key1, &ExtractionResult::new(key1))
            .await?;
        store
            .store_result(&key2, &ExtractionResult::new(key2))
            .await?;

        store.clear_all().await?;

        assert!(store.get_result(&key1).await?.is_none());
        assert!(store.get_result(&key2).await?.is_none());
        Ok(())
    }
}
