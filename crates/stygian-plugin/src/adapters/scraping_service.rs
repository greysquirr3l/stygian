//! `PluginExtractionAdapter`: implements `ScrapingService` for plugin-based extraction

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{debug, info};

use stygian_graph::ports::{ScrapingService, ServiceInput, ServiceOutput};
use stygian_graph::prelude::*;

use crate::IdempotencyKey;
use crate::domain::ExtractionRequest;
use crate::ports::{IdempotencyKeyStore, PluginExtractionPort, PluginTemplateStore};

/// Adapter implementing `ScrapingService` using plugin extraction
///
/// Bridges the plugin system into stygian-graph's service registry.
/// Can be selected in pipelines with `kind: "plugin"`.
///
/// # Example
///
/// ```no_run
/// use stygian_plugin::adapters::PluginExtractionAdapter;
/// use stygian_plugin::storage::{FileTemplateStore, MemoryIdempotencyStore};
/// use stygian_graph::ports::ScrapingService;
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let template_store = Arc::new(FileTemplateStore::new("./templates".into()));
/// let idempotency_store = Arc::new(MemoryIdempotencyStore::new());
/// let adapter = PluginExtractionAdapter::new(
///     template_store,
///     Arc::new(stygian_plugin::adapters::ExtractionEngine),
///     idempotency_store,
/// );
/// # Ok(())
/// # }
/// ```
pub struct PluginExtractionAdapter {
    template_store: Arc<dyn PluginTemplateStore>,
    extraction_port: Arc<dyn PluginExtractionPort>,
    idempotency_store: Arc<dyn IdempotencyKeyStore>,
}

impl PluginExtractionAdapter {
    /// Create a new plugin extraction adapter
    pub fn new(
        template_store: Arc<dyn PluginTemplateStore>,
        extraction_port: Arc<dyn PluginExtractionPort>,
        idempotency_store: Arc<dyn IdempotencyKeyStore>,
    ) -> Self {
        Self {
            template_store,
            extraction_port,
            idempotency_store,
        }
    }
}

#[async_trait]
impl ScrapingService for PluginExtractionAdapter {
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        debug!(
            "PluginExtractionAdapter executing: url={}, params={:?}",
            input.url, input.params
        );

        // Extract template ID and idempotency key from params
        let template_id = input
            .params
            .get("template_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                StygianError::Service(ServiceError::Unavailable(
                    "plugin extraction requires template_id in params".to_string(),
                ))
            })?;

        let idempotency_key_str = input.params.get("idempotency_key").and_then(Value::as_str);

        // Parse template ID
        let template_uuid = uuid::Uuid::parse_str(template_id).map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "invalid template_id: {e}"
            )))
        })?;

        // Retrieve template from store
        let template = self.template_store.get(&template_uuid).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "failed to load template: {e}"
            )))
        })?;

        // Create or reuse idempotency key
        let idempotency_key = if let Some(key_str) = idempotency_key_str {
            key_str.parse::<IdempotencyKey>().map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "invalid idempotency_key: {e}"
                )))
            })?
        } else {
            IdempotencyKey::new()
        };

        // Check for cached result (idempotency)
        if let Ok(Some(cached)) = self.idempotency_store.get_result(&idempotency_key).await {
            info!("Plugin extraction cache hit for key: {}", idempotency_key);
            return Ok(ServiceOutput {
                data: serde_json::to_string(&cached.data).unwrap_or_default(),
                metadata: json!({
                    "extraction": cached.data,
                    "metadata": cached.metadata,
                    "cached": true,
                }),
            });
        }

        // Determine HTML source: prefer params["html"], but can be passed via URL if it's full HTML
        let html = if let Some(html_str) = input.params.get("html").and_then(|v| v.as_str()) {
            // HTML explicitly provided in params (from fallback chain)
            html_str.to_string()
        } else if input.url.starts_with('<') {
            // URL field actually contains HTML (edge case)
            input.url.clone()
        } else {
            // No HTML available; cannot proceed
            return Err(StygianError::Service(ServiceError::Unavailable(
                "No HTML content provided in params['html'] or URL; plugin extraction cannot proceed".to_string(),
            )));
        };

        let request = ExtractionRequest::new(template, input.url.clone(), html)
            .with_idempotency_key(idempotency_key);

        // Execute extraction
        let result = self.extraction_port.execute(&request).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!("extraction failed: {e}")))
        })?;

        // Cache the result
        let _ = self
            .idempotency_store
            .store_result(&idempotency_key, &result)
            .await;

        info!("Plugin extraction completed: {} regions successful", {
            result
                .metadata
                .region_status
                .values()
                .filter(|s| s.success)
                .count()
        });

        Ok(ServiceOutput {
            data: serde_json::to_string(&result.data).unwrap_or_default(),
            metadata: json!({
                "extraction": result.data,
                "metadata": result.metadata,
                "cached": false,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "plugin-extraction"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{FileTemplateStore, MemoryIdempotencyStore};
    use crate::{
        adapters::ExtractionEngine,
        domain::{ExtractionTemplate, Region, Selector},
    };
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_adapter_executes_extraction()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let template_store = Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));
        let extraction_port = Arc::new(ExtractionEngine);
        let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

        // Create and save a template
        let region = Region::new("title", Selector::css("h1"), json!({"type": "string"}));
        let template = ExtractionTemplate::new("Test").with_region(region);
        let template_id = template.id;

        template_store.save(&template).await?;

        let adapter =
            PluginExtractionAdapter::new(template_store, extraction_port, idempotency_store);

        // Create input with template ID
        let input = ServiceInput {
            url: "<html><h1>Test Title</h1></html>".to_string(),
            params: json!({
                "template_id": template_id.to_string(),
            }),
        };

        let result = adapter.execute(input).await?;
        assert!(!result.data.is_empty());
        assert_eq!(result.metadata.get("cached"), Some(&json!(false)));
        Ok(())
    }

    #[tokio::test]
    async fn test_adapter_returns_cached_result()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let template_store = Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));
        let extraction_port = Arc::new(ExtractionEngine);
        let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

        // Create template
        let region = Region::new("title", Selector::css("h1"), json!({"type": "string"}));
        let template = ExtractionTemplate::new("Test").with_region(region);
        let template_id = template.id;

        template_store.save(&template).await?;

        let adapter =
            PluginExtractionAdapter::new(template_store, extraction_port, idempotency_store);

        let idempotency_key = IdempotencyKey::new();

        // First execution
        let input1 = ServiceInput {
            url: "<html><h1>Test</h1></html>".to_string(),
            params: json!({
                "template_id": template_id.to_string(),
                "idempotency_key": idempotency_key.to_string(),
            }),
        };

        let result1 = adapter.execute(input1).await?;
        assert_eq!(result1.metadata.get("cached"), Some(&json!(false)));

        // Second execution with same key should be cached
        let input2 = ServiceInput {
            url: "<html><h1>Different</h1></html>".to_string(),
            params: json!({
                "template_id": template_id.to_string(),
                "idempotency_key": idempotency_key.to_string(),
            }),
        };

        let result2 = adapter.execute(input2).await?;
        assert_eq!(result2.metadata.get("cached"), Some(&json!(true)));
        assert_eq!(result1.data, result2.data); // Should be identical (cached)
        Ok(())
    }
}
