//! Port trait definitions for stygian-plugin
//!
//! Defines interfaces that adapters must implement following hexagonal architecture.
//! The domain layer depends only on these traits, not on concrete implementations.

use crate::domain::{ExtractionRequest, ExtractionResult, ExtractionTemplate, IdempotencyKey};
use async_trait::async_trait;

/// Port for persisting and retrieving extraction templates
///
/// Implementations handle storage (file, database, cloud) and lifecycle management.
///
/// # Example
///
/// ```no_run
/// use stygian_plugin::ports::PluginTemplateStore;
/// use stygian_plugin::domain::ExtractionTemplate;
/// # async fn example(store: impl PluginTemplateStore) -> Result<(), Box<dyn std::error::Error>> {
/// let template = ExtractionTemplate::new("Product Extractor");
/// store.save(&template).await?;
/// let retrieved = store.get(&template.id).await?;
/// assert_eq!(template.id, retrieved.id);
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait PluginTemplateStore: Send + Sync {
    /// Save or update a template
    async fn save(&self, template: &ExtractionTemplate) -> crate::Result<()>;

    /// Retrieve a template by ID
    async fn get(&self, id: &uuid::Uuid) -> crate::Result<ExtractionTemplate>;

    /// List all templates
    async fn list(&self) -> crate::Result<Vec<ExtractionTemplate>>;

    /// Delete a template by ID
    async fn delete(&self, id: &uuid::Uuid) -> crate::Result<()>;

    /// Search templates by name (substring match)
    async fn search(&self, query: &str) -> crate::Result<Vec<ExtractionTemplate>> {
        let all = self.list().await?;
        let results = all
            .into_iter()
            .filter(|t| t.name.to_lowercase().contains(&query.to_lowercase()))
            .collect();
        Ok(results)
    }
}

/// Port for executing data extraction on a page
///
/// Implementations handle DOM interaction (via browser automation or plugin content script).
///
/// # Example
///
/// ```no_run
/// use stygian_plugin::ports::PluginExtractionPort;
/// use stygian_plugin::domain::{ExtractionRequest, ExtractionTemplate};
/// # async fn example(port: impl PluginExtractionPort) -> Result<(), Box<dyn std::error::Error>> {
/// let template = ExtractionTemplate::new("Example");
/// let request = ExtractionRequest::new(template, "https://example.com", "<html>...</html>");
/// let result = port.execute(&request).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait PluginExtractionPort: Send + Sync {
    /// Execute extraction using a request
    async fn execute(&self, request: &ExtractionRequest) -> crate::Result<ExtractionResult>;

    /// Validate a selector against the current/provided DOM
    async fn validate_selector(
        &self,
        _html: &str,
        _selector_expr: &str,
    ) -> crate::Result<(bool, usize)> {
        // Default implementation: always returns (true, 0) for estimation
        // Concrete implementations should validate against actual DOM
        Ok((true, 0))
    }
}

/// Port for tracking idempotency keys and results
///
/// Prevents duplicate extractions by caching results per idempotency key.
///
/// # Example
///
/// ```no_run
/// use stygian_plugin::ports::IdempotencyKeyStore;
/// use stygian_plugin::domain::{IdempotencyKey, ExtractionResult};
/// # async fn example(store: impl IdempotencyKeyStore) -> Result<(), Box<dyn std::error::Error>> {
/// let key = IdempotencyKey::new();
/// let result = ExtractionResult::new(key);
/// store.store_result(&key, &result).await?;
/// let retrieved = store.get_result(&key).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait IdempotencyKeyStore: Send + Sync {
    /// Store an extraction result under an idempotency key
    async fn store_result(
        &self,
        key: &IdempotencyKey,
        result: &ExtractionResult,
    ) -> crate::Result<()>;

    /// Retrieve a cached result by idempotency key
    async fn get_result(&self, key: &IdempotencyKey) -> crate::Result<Option<ExtractionResult>>;

    /// Delete an old result (cleanup)
    async fn delete_result(&self, key: &IdempotencyKey) -> crate::Result<()>;

    /// Clear all results (for testing)
    async fn clear_all(&self) -> crate::Result<()>;
}

/// Optional: Recording port for capturing user interactions and generating templates
///
/// Implementations handle recording DOM interactions, building selectors, and inferring templates.
///
/// Not required for basic extraction; useful for the interactive recording mode.
#[async_trait]
pub trait PluginRecordingPort: Send + Sync {
    /// Start recording user interactions
    async fn start_recording(&self) -> crate::Result<String>;

    /// Record an element selection
    async fn record_element_click(
        &self,
        session_id: &str,
        element_info: serde_json::Value,
    ) -> crate::Result<()>;

    /// Finalize recording and generate a template
    async fn finalize_recording(
        &self,
        session_id: &str,
        template_name: &str,
    ) -> crate::Result<ExtractionTemplate>;

    /// Cancel an active recording
    async fn cancel_recording(&self, session_id: &str) -> crate::Result<()>;
}
