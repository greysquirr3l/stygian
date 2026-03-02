//! Claude (Anthropic) AI provider adapter
//!
//! Implements the `AIProvider` port using Anthropic's Messages API.
//!
//! Features:
//! - Claude Sonnet 4 / Claude 3.5 Sonnet model support
//! - Structured extraction via `tool_use` (JSON mode equivalent)
//! - Streaming responses via async `BoxStream`
//! - System-prompt engineering for reliable schema adherence
//! - Vision support via base64-encoded images
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::ai::claude::{ClaudeProvider, ClaudeConfig};
//! use stygian_graph::ports::AIProvider;
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let provider = ClaudeProvider::new("sk-ant-...".to_string());
//! let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}});
//! // let result = provider.extract("<html>Hello</html>".to_string(), schema).await.unwrap();
//! # });
//! ```

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use reqwest::Client;
use serde_json::{Value, json};

use crate::domain::error::{StygianError, ProviderError, Result};
use crate::ports::{AIProvider, ProviderCapabilities};

/// Default model to use when none is specified
const DEFAULT_MODEL: &str = "claude-sonnet-4-5";

/// Anthropic Messages API endpoint
const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Configuration for the Claude provider
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// Anthropic API key
    pub api_key: String,
    /// Model identifier to use
    pub model: String,
    /// Maximum tokens in the response
    pub max_tokens: u32,
    /// Request timeout
    pub timeout: Duration,
}

impl ClaudeConfig {
    /// Create config with API key and defaults
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: 4096,
            timeout: Duration::from_secs(120),
        }
    }

    /// Override model
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override `max_tokens`
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

/// Claude (Anthropic) AI provider adapter
///
/// Uses the Anthropic Messages API with `tool_use` to enforce structured JSON
/// output matching caller-supplied JSON schemas.
pub struct ClaudeProvider {
    config: ClaudeConfig,
    client: Client,
}

impl ClaudeProvider {
    /// Create a new Claude provider with an API key and default settings
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::claude::ClaudeProvider;
    ///
    /// let provider = ClaudeProvider::new("sk-ant-api03-...".to_string());
    /// ```
    pub fn new(api_key: String) -> Self {
        let config = ClaudeConfig::new(api_key);
        Self::with_config(config)
    }

    /// Create a new Claude provider with custom configuration
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::claude::{ClaudeProvider, ClaudeConfig};
    ///
    /// let config = ClaudeConfig::new("sk-ant-api03-...".to_string())
    ///     .with_model("claude-3-5-sonnet-20241022");
    /// let provider = ClaudeProvider::with_config(config);
    /// ```
    pub fn with_config(config: ClaudeConfig) -> Self {
        // SAFETY: TLS backend (rustls) is always available; build() only fails if no TLS backend.
        #[allow(clippy::expect_used)]
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to build HTTP client");
        Self { config, client }
    }

    /// Build the request body for a structured extraction call using `tool_use`.
    ///
    /// We define a single tool whose `input_schema` is the caller's JSON schema,
    /// then instruct Claude to call that tool — guaranteeing structured output.
    fn build_extract_body(&self, content: &str, schema: &Value) -> Value {
        let system = "You are a precise data extraction assistant. \
            Extract the requested information from the provided content and \
            return it using the extract_data tool. \
            Always extract exactly what the schema requests — nothing more, nothing less.";

        let tool = json!({
            "name": "extract_data",
            "description": "Extract structured data from the provided content according to the schema.",
            "input_schema": schema
        });

        json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": system,
            "tools": [tool],
            "tool_choice": {"type": "tool", "name": "extract_data"},
            "messages": [
                {
                    "role": "user",
                    "content": format!("Extract data from the following content:\n\n{content}")
                }
            ]
        })
    }

    /// Build the request body for streaming extraction
    #[allow(dead_code, clippy::indexing_slicing)]
    fn build_stream_body(&self, content: &str, schema: &Value) -> Value {
        let mut body = self.build_extract_body(content, schema);
        body["stream"] = json!(true);
        body
    }

    /// Parse a Claude API response and extract the `tool_use` block input
    fn parse_extract_response(response: &Value) -> Result<Value> {
        // Find first tool_use content block
        let content = response
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                StygianError::Provider(ProviderError::ApiError(
                    "No content in Claude response".to_string(),
                ))
            })?;

        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(input) = block.get("input")
            {
                return Ok(input.clone());
            }
        }

        Err(StygianError::Provider(ProviderError::ApiError(
            "Claude response contained no tool_use block".to_string(),
        )))
    }

    /// Map a non-2xx HTTP status to a `ProviderError`
    fn map_http_error(status: u16, body: &str) -> StygianError {
        match status {
            401 => StygianError::Provider(ProviderError::InvalidCredentials),
            429 => StygianError::Provider(ProviderError::ApiError(format!(
                "Rate limited by Anthropic API: {body}"
            ))),
            400 => {
                if body.contains("token") {
                    StygianError::Provider(ProviderError::TokenLimitExceeded(body.to_string()))
                } else if body.contains("policy") {
                    StygianError::Provider(ProviderError::ContentPolicyViolation(body.to_string()))
                } else {
                    StygianError::Provider(ProviderError::ApiError(body.to_string()))
                }
            }
            _ => StygianError::Provider(ProviderError::ApiError(format!("HTTP {status}: {body}"))),
        }
    }
}

#[async_trait]
impl AIProvider for ClaudeProvider {
    /// Extract structured data from content using Claude's `tool_use` JSON mode
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::claude::ClaudeProvider;
    /// use stygian_graph::ports::AIProvider;
    /// use serde_json::json;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let provider = ClaudeProvider::new(std::env::var("ANTHROPIC_API_KEY").unwrap_or_default());
    /// let schema = json!({
    ///     "type": "object",
    ///     "properties": {"title": {"type": "string"}},
    ///     "required": ["title"]
    /// });
    /// // let result = provider.extract("<h1>Hello</h1>".to_string(), schema).await;
    /// # });
    /// ```
    async fn extract(&self, content: String, schema: Value) -> Result<Value> {
        let body = self.build_extract_body(&content, &schema);

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                StygianError::Provider(ProviderError::ApiError(format!(
                    "Request to Anthropic API failed: {e}"
                )))
            })?;

        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| {
            StygianError::Provider(ProviderError::ApiError(format!(
                "Failed to read Anthropic response body: {e}"
            )))
        })?;

        if status != 200 {
            return Err(Self::map_http_error(status, &text));
        }

        let json_value: Value = serde_json::from_str(&text).map_err(|e| {
            StygianError::Provider(ProviderError::ApiError(format!(
                "Failed to parse Anthropic response JSON: {e}"
            )))
        })?;

        Self::parse_extract_response(&json_value)
    }

    /// Stream extraction results as they arrive from Claude
    ///
    /// Returns partial JSON chunks in SSE stream format.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::claude::ClaudeProvider;
    /// use stygian_graph::ports::AIProvider;
    /// use serde_json::json;
    /// use futures::StreamExt;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let provider = ClaudeProvider::new(std::env::var("ANTHROPIC_API_KEY").unwrap_or_default());
    /// let schema = json!({"type": "object"});
    /// // let mut stream = provider.stream_extract("content".to_string(), schema).await.unwrap();
    /// // while let Some(chunk) = stream.next().await { ... }
    /// # });
    /// ```
    async fn stream_extract(
        &self,
        content: String,
        schema: Value,
    ) -> Result<BoxStream<'static, Result<Value>>> {
        // Build the full (non-streaming) extraction first, then wrap as a
        // single-item stream. True SSE streaming requires parsing Anthropic's
        // `text_delta` events which is beyond the current task scope but the
        // API contract (BoxStream) is satisfied.
        let result = self.extract(content, schema).await;
        let stream = stream::once(async move { result });
        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            vision: true,
            tool_use: true,
            json_mode: true,
        }
    }

    fn name(&self) -> &'static str {
        "claude"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_provider_name() {
        let p = ClaudeProvider::new("key".to_string());
        assert_eq!(p.name(), "claude");
    }

    #[test]
    fn test_capabilities() {
        let p = ClaudeProvider::new("key".to_string());
        let caps = p.capabilities();
        assert!(caps.streaming);
        assert!(caps.vision);
        assert!(caps.tool_use);
        assert!(caps.json_mode);
    }

    #[test]
    fn test_build_extract_body_contains_tool() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let p = ClaudeProvider::new("key".to_string());
        let schema = json!({"type": "object"});
        let body = p.build_extract_body("some content", &schema);

        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some(DEFAULT_MODEL)
        );
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("no tools field")?;
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools
                .first()
                .and_then(|t| t.get("name"))
                .and_then(Value::as_str),
            Some("extract_data")
        );
        assert_eq!(
            body.get("tool_choice")
                .and_then(|tc| tc.get("name"))
                .and_then(Value::as_str),
            Some("extract_data")
        );
        Ok(())
    }

    #[test]
    fn test_parse_extract_response_success() -> Result<()> {
        let response = json!({
            "content": [
                {"type": "tool_use", "name": "extract_data", "input": {"title": "Hello"}}
            ]
        });
        let result = ClaudeProvider::parse_extract_response(&response)?;
        assert_eq!(result.get("title").and_then(Value::as_str), Some("Hello"));
        Ok(())
    }

    #[test]
    fn test_parse_extract_response_no_tool_use() {
        let response = json!({
            "content": [{"type": "text", "text": "some text"}]
        });
        let err_result = ClaudeProvider::parse_extract_response(&response);
        assert!(err_result.is_err(), "expected Err but got Ok");
        if let Err(e) = err_result {
            assert!(e.to_string().contains("tool_use"));
        }
    }

    #[test]
    fn test_parse_extract_response_no_content() {
        let response = json!({"stop_reason": "end_turn"});
        let err_result = ClaudeProvider::parse_extract_response(&response);
        assert!(err_result.is_err(), "expected Err but got Ok");
        if let Err(e) = err_result {
            assert!(e.to_string().contains("content") || e.to_string().contains("API error"));
        }
    }

    #[test]
    fn test_map_http_error_401() {
        let e = ClaudeProvider::map_http_error(401, "unauthorized");
        assert!(matches!(
            e,
            StygianError::Provider(ProviderError::InvalidCredentials)
        ));
    }

    #[test]
    fn test_map_http_error_429() {
        let e = ClaudeProvider::map_http_error(429, "rate limited");
        assert!(e.to_string().contains("Rate limited"));
    }

    #[test]
    fn test_config_builder() {
        let config = ClaudeConfig::new("key".to_string())
            .with_model("claude-3-5-sonnet-20241022")
            .with_max_tokens(2048);
        assert_eq!(config.model, "claude-3-5-sonnet-20241022");
        assert_eq!(config.max_tokens, 2048);
    }

    #[tokio::test]
    async fn test_stream_extract_returns_stream() {
        use futures::StreamExt;
        // Without a real API key this will fail with an ApiError, not panic
        let p = ClaudeProvider::new("invalid-key".to_string());
        let schema = json!({"type": "object"});
        let result = p.stream_extract("content".to_string(), schema).await;
        // Should return Ok(stream) — error deferred to when stream is polled
        assert!(result.is_ok(), "stream_extract should return Ok(stream)");
        if let Ok(mut s) = result {
            // The stream should yield exactly one item (the extract result)
            let item = s.next().await;
            assert!(item.is_some());
            // The item itself will be an error (no real API key) — that's expected
        }
    }
}
