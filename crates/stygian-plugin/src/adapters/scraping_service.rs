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
use crate::reliability::ReliabilityScorer;

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

        let (template, idempotency_key) = self.resolve_inputs(&input).await?;

        if let Some(cached_output) = self.try_cache_hit(&idempotency_key).await {
            return Ok(cached_output);
        }

        let html = extract_html_from_input(&input)?;
        let request = ExtractionRequest::new(template, input.url.clone(), html)
            .with_idempotency_key(idempotency_key);

        let mut result = self.extraction_port.execute(&request).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!("extraction failed: {e}")))
        })?;

        self.finalize_result(&idempotency_key, &mut result).await
    }

    fn name(&self) -> &'static str {
        "plugin-extraction"
    }
}

impl PluginExtractionAdapter {
    /// Parse the `template_id` and `idempotency_key` params and load the
    /// template from the store.
    async fn resolve_inputs(
        &self,
        input: &ServiceInput,
    ) -> Result<(crate::domain::ExtractionTemplate, IdempotencyKey)> {
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

        let template_uuid = uuid::Uuid::parse_str(template_id).map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "invalid template_id: {e}"
            )))
        })?;

        let template = self.template_store.get(&template_uuid).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "failed to load template: {e}"
            )))
        })?;

        let idempotency_key = if let Some(key_str) = idempotency_key_str {
            key_str.parse::<IdempotencyKey>().map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "invalid idempotency_key: {e}"
                )))
            })?
        } else {
            IdempotencyKey::new()
        };

        Ok((template, idempotency_key))
    }

    /// Check the idempotency store for a cached result and, if present,
    /// build the corresponding `ServiceOutput` (including the T87
    /// reliability score).
    async fn try_cache_hit(&self, idempotency_key: &IdempotencyKey) -> Option<ServiceOutput> {
        let cached = match self.idempotency_store.get_result(idempotency_key).await {
            Ok(Some(cached)) => cached,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(
                    "idempotency lookup failed for key {}: {}",
                    idempotency_key,
                    e
                );
                return None;
            }
        };
        info!("Plugin extraction cache hit for key: {}", idempotency_key);
        let cached_score = cached
            .metadata
            .reliability
            .clone()
            .unwrap_or_else(|| ReliabilityScorer::new().score_extraction(&cached, 0));
        Some(ServiceOutput {
            data: serde_json::to_string(&cached.data).unwrap_or_default(),
            metadata: json!({
                "extraction": cached.data,
                "metadata": cached.metadata,
                "reliability": cached_score,
                "cached": true,
            }),
        })
    }

    /// Attach the T87 reliability score to the result, persist it in the
    /// idempotency cache, and assemble the final `ServiceOutput`.
    async fn finalize_result(
        &self,
        idempotency_key: &IdempotencyKey,
        result: &mut crate::domain::ExtractionResult,
    ) -> Result<ServiceOutput> {
        let score = ReliabilityScorer::new().score_extraction(result, 0);
        result.metadata.reliability = Some(score.clone());

        if let Err(e) = self
            .idempotency_store
            .store_result(idempotency_key, result)
            .await
        {
            tracing::warn!(
                "failed to store idempotent result for key {}: {}",
                idempotency_key,
                e
            );
        }

        let successful_regions = result
            .metadata
            .region_status
            .values()
            .filter(|s| s.success)
            .count();
        info!(
            "Plugin extraction completed: {} regions successful, reliability={:.3} ({})",
            successful_regions, score.overall, score.band
        );

        Ok(ServiceOutput {
            data: serde_json::to_string(&result.data).unwrap_or_default(),
            metadata: json!({
                "extraction": result.data,
                "metadata": result.metadata,
                "reliability": score,
                "cached": false,
            }),
        })
    }
}

/// Pull HTML for extraction from the [`ServiceInput`].
///
/// Prefers `params["html"]` (the fallback-chain flow), but accepts the URL
/// field directly when it looks like inline HTML. Returns an error when
/// neither source is available.
fn extract_html_from_input(input: &ServiceInput) -> Result<String> {
    if let Some(html_str) = input.params.get("html").and_then(|v| v.as_str()) {
        return Ok(html_str.to_string());
    }
    if input.url.starts_with('<') {
        return Ok(input.url.clone());
    }
    Err(StygianError::Service(ServiceError::Unavailable(
        "No HTML content provided in params['html'] or URL; plugin extraction cannot proceed"
            .to_string(),
    )))
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
