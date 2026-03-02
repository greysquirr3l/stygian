//! LLM Extraction Service — orchestrator that uses AIProvider to extract structured data
//!
//! Implements `ScrapingService` by delegating to one or more `AIProvider`s.
//! Supports provider fallback: tries providers in order until one succeeds.
//!
//! # Architecture
//!
//! ```text
//! ScrapingService  ←  LlmExtractionService  →  AIProvider (Claude, GPT, Gemini, …)
//!       ↑                     ↓
//!  ServiceInput           FallbackChain
//!  { url, params }     [primary, secondary, …]
//! ```
//!
//! The `params` field of `ServiceInput` must contain:
//! - `schema`: JSON schema object defining the expected output shape
//! - `content` (optional): If present, used as-is. Otherwise `data` from a prior
//!   pipeline stage should be passed via `url`.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::application::extraction::{LlmExtractionService, ExtractionConfig};
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! // Provider built separately — inject via Arc<dyn AIProvider>
//! // let service = LlmExtractionService::new(providers, ExtractionConfig::default());
//! // let output = service.execute(input).await.unwrap();
//! # });
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::domain::error::{StygianError, ProviderError, Result};
use crate::ports::{AIProvider, ScrapingService, ServiceInput, ServiceOutput};

/// Configuration for the LLM extraction service
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    /// Maximum content length sent to providers (characters).
    /// Content is truncated at this limit to avoid token overflow.
    pub max_content_chars: usize,
    /// Whether to validate the provider output against the schema.
    /// Currently performs a structural check (is the output a JSON object?).
    pub validate_output: bool,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            max_content_chars: 64_000,
            validate_output: true,
        }
    }
}

/// LLM-based structured data extraction service
///
/// Wraps one or more `AIProvider` instances and implements `ScrapingService`.
/// On each `execute()` call the service:
///
/// 1. Reads `schema` and optionally `content` from `ServiceInput.params`.
/// 2. Iterates through the provider list until one returns `Ok`.
/// 3. Returns extracted data in `ServiceOutput.data` (as JSON string).
/// 4. Metadata includes which provider succeeded and elapsed time.
///
/// # Provider Fallback
///
/// Providers are tried **in the order they were added**. The first success
/// short-circuits the chain. Errors from skipped providers are logged as
/// warnings, not propagated.
pub struct LlmExtractionService {
    /// Ordered fallback chain of AI providers
    providers: Vec<Arc<dyn AIProvider>>,
    config: ExtractionConfig,
}

impl LlmExtractionService {
    /// Create a new extraction service with an ordered fallback chain
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::application::extraction::{LlmExtractionService, ExtractionConfig};
    /// use stygian_graph::adapters::ai::ollama::OllamaProvider;
    /// use std::sync::Arc;
    ///
    /// let providers: Vec<Arc<dyn stygian_graph::ports::AIProvider>> = vec![
    ///     Arc::new(OllamaProvider::new()),
    /// ];
    /// let service = LlmExtractionService::new(providers, ExtractionConfig::default());
    /// ```
    pub fn new(providers: Vec<Arc<dyn AIProvider>>, config: ExtractionConfig) -> Self {
        Self { providers, config }
    }

    /// Resolve the content to extract from.
    ///
    /// Priority:
    /// 1. `params["content"]` if present
    /// 2. `input.url` as fallback (useful when this node receives raw HTML from prior stage)
    fn resolve_content(input: &ServiceInput) -> &str {
        input
            .params
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or(&input.url)
    }

    /// Truncate content to the configured character limit
    fn truncate_content<'a>(&self, content: &'a str) -> &'a str {
        if content.len() <= self.config.max_content_chars {
            content
        } else {
            warn!(
                limit = self.config.max_content_chars,
                actual = content.len(),
                "Content truncated for LLM extraction"
            );
            &content[..self.config.max_content_chars]
        }
    }

    /// Extract the `schema` from params, returning an error if missing
    fn resolve_schema(input: &ServiceInput) -> Result<Value> {
        input.params.get("schema").cloned().ok_or_else(|| {
            StygianError::Provider(ProviderError::ApiError(
                "LlmExtractionService requires 'schema' in ServiceInput.params".to_string(),
            ))
        })
    }

    /// Validate that extracted output is a JSON object (basic schema check)
    fn validate_output(output: &Value) -> Result<()> {
        if output.is_object() || output.is_array() {
            Ok(())
        } else {
            Err(StygianError::Provider(ProviderError::ApiError(format!(
                "Provider returned non-object output: {output}"
            ))))
        }
    }
}

#[async_trait]
impl ScrapingService for LlmExtractionService {
    /// Execute structured extraction via the provider fallback chain
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::application::extraction::{LlmExtractionService, ExtractionConfig};
    /// use stygian_graph::adapters::ai::ollama::OllamaProvider;
    /// use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// use serde_json::json;
    /// use std::sync::Arc;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let providers: Vec<Arc<dyn stygian_graph::ports::AIProvider>> = vec![
    ///     Arc::new(OllamaProvider::new()),
    /// ];
    /// let service = LlmExtractionService::new(providers, ExtractionConfig::default());
    /// let input = ServiceInput {
    ///     url: "<h1>Hello World</h1>".to_string(),
    ///     params: json!({
    ///         "schema": {"type": "object", "properties": {"heading": {"type": "string"}}},
    ///     }),
    /// };
    /// // let output = service.execute(input).await.unwrap();
    /// # });
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        if self.providers.is_empty() {
            return Err(StygianError::Provider(ProviderError::ApiError(
                "No AI providers configured in LlmExtractionService".to_string(),
            )));
        }

        let schema = Self::resolve_schema(&input)?;
        let raw_content = Self::resolve_content(&input);
        let content = self.truncate_content(raw_content).to_string();

        let start = std::time::Instant::now();
        let mut last_error: Option<StygianError> = None;

        for provider in &self.providers {
            debug!(provider = provider.name(), "Attempting LLM extraction");

            match provider.extract(content.clone(), schema.clone()).await {
                Ok(extracted) => {
                    if self.config.validate_output
                        && let Err(e) = Self::validate_output(&extracted)
                    {
                        warn!(
                            provider = provider.name(),
                            error = %e,
                            "Provider returned invalid output, trying next"
                        );
                        last_error = Some(e);
                        continue;
                    }

                    let elapsed = start.elapsed();
                    info!(
                        provider = provider.name(),
                        elapsed_ms = elapsed.as_millis(),
                        "LLM extraction succeeded"
                    );

                    return Ok(ServiceOutput {
                        data: extracted.to_string(),
                        metadata: json!({
                            "provider": provider.name(),
                            "elapsed_ms": elapsed.as_millis(),
                            "content_chars": content.len(),
                        }),
                    });
                }
                Err(e) => {
                    warn!(
                        provider = provider.name(),
                        error = %e,
                        "Provider failed, trying next in chain"
                    );
                    last_error = Some(e);
                }
            }
        }

        // All providers failed
        Err(last_error.unwrap_or_else(|| {
            StygianError::Provider(ProviderError::ApiError(
                "All AI providers in fallback chain failed".to_string(),
            ))
        }))
    }

    fn name(&self) -> &'static str {
        "llm-extraction"
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value
)]
mod tests {
    use super::*;
    use crate::ports::ProviderCapabilities;
    use futures::stream::{self, BoxStream};
    use serde_json::json;

    // --- Mock AIProvider for tests ---

    struct AlwaysSucceed {
        response: Value,
    }

    #[async_trait]
    impl AIProvider for AlwaysSucceed {
        async fn extract(&self, _content: String, _schema: Value) -> Result<Value> {
            Ok(self.response.clone())
        }

        async fn stream_extract(
            &self,
            _content: String,
            _schema: Value,
        ) -> Result<BoxStream<'static, Result<Value>>> {
            Ok(Box::pin(stream::once(async { Ok(json!({})) })))
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn name(&self) -> &'static str {
            "mock-succeed"
        }
    }

    struct AlwaysFail;

    #[async_trait]
    impl AIProvider for AlwaysFail {
        async fn extract(&self, _content: String, _schema: Value) -> Result<Value> {
            Err(StygianError::Provider(ProviderError::ApiError(
                "mock failure".to_string(),
            )))
        }

        async fn stream_extract(
            &self,
            _content: String,
            _schema: Value,
        ) -> Result<BoxStream<'static, Result<Value>>> {
            Err(StygianError::Provider(ProviderError::ApiError(
                "mock failure".to_string(),
            )))
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn name(&self) -> &'static str {
            "mock-fail"
        }
    }

    fn make_input(schema: Value) -> ServiceInput {
        ServiceInput {
            url: "<h1>Hello</h1>".to_string(),
            params: json!({ "schema": schema }),
        }
    }

    #[tokio::test]
    async fn test_service_name() {
        let svc = LlmExtractionService::new(vec![], ExtractionConfig::default());
        assert_eq!(svc.name(), "llm-extraction");
    }

    #[tokio::test]
    async fn test_no_providers_returns_error() {
        let svc = LlmExtractionService::new(vec![], ExtractionConfig::default());
        let err = svc.execute(make_input(json!({}))).await.unwrap_err();
        assert!(err.to_string().contains("No AI providers"));
    }

    #[tokio::test]
    async fn test_missing_schema_returns_error() {
        let providers: Vec<Arc<dyn AIProvider>> = vec![Arc::new(AlwaysSucceed {
            response: json!({}),
        })];
        let svc = LlmExtractionService::new(providers, ExtractionConfig::default());
        let input = ServiceInput {
            url: "some content".to_string(),
            params: json!({}), // no schema key
        };
        let err = svc.execute(input).await.unwrap_err();
        assert!(err.to_string().contains("schema"));
    }

    #[tokio::test]
    async fn test_single_succeeding_provider() {
        let providers: Vec<Arc<dyn AIProvider>> = vec![Arc::new(AlwaysSucceed {
            response: json!({"title": "Hello"}),
        })];
        let svc = LlmExtractionService::new(providers, ExtractionConfig::default());
        let output = svc.execute(make_input(json!({}))).await.unwrap();
        assert_eq!(
            output.metadata["provider"].as_str().unwrap(),
            "mock-succeed"
        );
        let data: Value = serde_json::from_str(&output.data).unwrap();
        assert_eq!(data["title"].as_str().unwrap(), "Hello");
    }

    #[tokio::test]
    async fn test_fallback_to_second_provider() {
        let providers: Vec<Arc<dyn AIProvider>> = vec![
            Arc::new(AlwaysFail),
            Arc::new(AlwaysSucceed {
                response: json!({"score": 42}),
            }),
        ];
        let svc = LlmExtractionService::new(providers, ExtractionConfig::default());
        let output = svc.execute(make_input(json!({}))).await.unwrap();
        assert_eq!(
            output.metadata["provider"].as_str().unwrap(),
            "mock-succeed"
        );
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let providers: Vec<Arc<dyn AIProvider>> = vec![Arc::new(AlwaysFail), Arc::new(AlwaysFail)];
        let svc = LlmExtractionService::new(providers, ExtractionConfig::default());
        let err = svc.execute(make_input(json!({}))).await.unwrap_err();
        assert!(err.to_string().contains("mock failure"));
    }

    #[tokio::test]
    async fn test_content_from_params_overrides_url() {
        let providers: Vec<Arc<dyn AIProvider>> = vec![Arc::new(AlwaysSucceed {
            response: json!({"ok": true}),
        })];
        let svc = LlmExtractionService::new(providers, ExtractionConfig::default());
        let input = ServiceInput {
            url: "should-not-be-used".to_string(),
            params: json!({
                "schema": {"type": "object"},
                "content": "actual content here"
            }),
        };
        let output = svc.execute(input).await.unwrap();
        // Metadata should reflect char count of "actual content here" (19 chars)
        assert_eq!(output.metadata["content_chars"].as_u64().unwrap(), 19);
    }

    #[test]
    fn test_truncate_content_short() {
        let svc = LlmExtractionService::new(vec![], ExtractionConfig::default());
        let s = "hello";
        assert_eq!(svc.truncate_content(s), s);
    }

    #[test]
    fn test_truncate_content_long() {
        let svc = LlmExtractionService::new(
            vec![],
            ExtractionConfig {
                max_content_chars: 5,
                ..Default::default()
            },
        );
        assert_eq!(svc.truncate_content("hello world"), "hello");
    }

    #[test]
    fn test_validate_output_object_ok() {
        assert!(LlmExtractionService::validate_output(&json!({"k": "v"})).is_ok());
    }

    #[test]
    fn test_validate_output_array_ok() {
        assert!(LlmExtractionService::validate_output(&json!([1, 2, 3])).is_ok());
    }

    #[test]
    fn test_validate_output_scalar_err() {
        assert!(LlmExtractionService::validate_output(&json!("just a string")).is_err());
    }
}
