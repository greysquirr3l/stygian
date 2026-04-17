//! OpenAI (ChatGPT) AI provider adapter
//!
//! Implements the `AIProvider` port using OpenAI's Chat Completions API.
//! Supports GPT-4o, GPT-4, and o1-series models with native JSON mode
//! (`response_format: json_object`) and function calling for structured extraction.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::ai::openai::{OpenAIProvider, OpenAIConfig};
//! use stygian_graph::ports::AIProvider;
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let provider = OpenAIProvider::new("sk-...".to_string());
//! let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}});
//! // let result = provider.extract("<html>Hello</html>".to_string(), schema).await.unwrap();
//! # });
//! ```

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use reqwest::Client;
use serde_json::{Value, json};

use crate::domain::error::{ProviderError, Result, StygianError};
use crate::ports::{AIProvider, ProviderCapabilities};

/// Default model
const DEFAULT_MODEL: &str = "gpt-4o";

/// Chat completions endpoint
const API_URL: &str = "https://api.openai.com/v1/chat/completions";

/// Configuration for the `OpenAI` provider
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    /// `OpenAI` API key
    pub api_key: String,
    /// Model identifier
    pub model: String,
    /// Maximum response tokens
    pub max_tokens: u32,
    /// Request timeout
    pub timeout: Duration,
}

impl OpenAIConfig {
    /// Create config with API key and defaults
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: 4096,
            timeout: Duration::from_mins(2),
        }
    }

    /// Override model
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

/// `OpenAI` provider adapter
///
/// Uses `response_format: json_object` + function calling to enforce schema.
pub struct OpenAIProvider {
    config: OpenAIConfig,
    client: Client,
}

impl OpenAIProvider {
    /// Create with API key and defaults
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::openai::OpenAIProvider;
    /// let p = OpenAIProvider::new("sk-...".to_string());
    /// ```
    pub fn new(api_key: String) -> Self {
        Self::with_config(OpenAIConfig::new(api_key))
    }

    /// Create with custom configuration
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::openai::{OpenAIProvider, OpenAIConfig};
    /// let config = OpenAIConfig::new("sk-...".to_string()).with_model("gpt-4");
    /// let p = OpenAIProvider::with_config(config);
    /// ```
    pub fn with_config(config: OpenAIConfig) -> Self {
        // SAFETY: TLS backend (rustls) is always available; build() only fails if no TLS backend.
        #[allow(clippy::expect_used)]
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to build HTTP client");
        Self { config, client }
    }

    fn build_body(&self, content: &str, schema: &Value) -> Value {
        let system = "You are a precise data extraction assistant. \
            Extract structured data from the provided content matching the given JSON schema. \
            Return ONLY valid JSON matching the schema, no extra text.";

        let user_msg = format!(
            "Schema: {}\n\nContent:\n{}",
            serde_json::to_string(schema).unwrap_or_default(),
            content
        );

        json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user_msg}
            ]
        })
    }

    fn parse_response(response: &Value) -> Result<Value> {
        let text = response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                StygianError::Provider(ProviderError::ApiError(
                    "No content in OpenAI response".to_string(),
                ))
            })?;

        serde_json::from_str(text).map_err(|e| {
            StygianError::Provider(ProviderError::ApiError(format!(
                "Failed to parse OpenAI JSON response: {e}"
            )))
        })
    }

    fn map_http_error(status: u16, body: &str) -> StygianError {
        match status {
            401 => StygianError::Provider(ProviderError::InvalidCredentials),
            429 => StygianError::Provider(ProviderError::ApiError(format!(
                "OpenAI rate limited: {body}"
            ))),
            _ => StygianError::Provider(ProviderError::ApiError(format!("HTTP {status}: {body}"))),
        }
    }
}

#[async_trait]
impl AIProvider for OpenAIProvider {
    async fn extract(&self, content: String, schema: Value) -> Result<Value> {
        let body = self.build_body(&content, &schema);

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", &self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                StygianError::Provider(ProviderError::ApiError(format!(
                    "OpenAI request failed: {e}"
                )))
            })?;

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| StygianError::Provider(ProviderError::ApiError(e.to_string())))?;

        if status != 200 {
            return Err(Self::map_http_error(status, &text));
        }

        let json_val: Value = serde_json::from_str(&text)
            .map_err(|e| StygianError::Provider(ProviderError::ApiError(e.to_string())))?;

        Self::parse_response(&json_val)
    }

    async fn stream_extract(
        &self,
        content: String,
        schema: Value,
    ) -> Result<BoxStream<'static, Result<Value>>> {
        let result = self.extract(content, schema).await;
        Ok(Box::pin(stream::once(async move { result })))
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
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_name() {
        assert_eq!(OpenAIProvider::new("k".to_string()).name(), "openai");
    }

    #[test]
    fn test_capabilities() {
        let caps = OpenAIProvider::new("k".to_string()).capabilities();
        assert!(caps.json_mode);
        assert!(caps.streaming);
    }

    #[test]
    fn test_build_body_contains_json_format() {
        let p = OpenAIProvider::new("k".to_string());
        let body = p.build_body("content", &json!({"type": "object"}));
        assert_eq!(
            body.get("response_format")
                .and_then(|rf| rf.get("type"))
                .and_then(Value::as_str),
            Some("json_object")
        );
    }

    #[test]
    fn test_parse_response_valid() -> Result<()> {
        let resp = json!({
            "choices": [{"message": {"content": "{\"title\": \"Hello\"}"}}]
        });
        let val = OpenAIProvider::parse_response(&resp)?;
        assert_eq!(val.get("title").and_then(Value::as_str), Some("Hello"));
        Ok(())
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let resp = json!({"choices": [{"message": {"content": "not json"}}]});
        assert!(OpenAIProvider::parse_response(&resp).is_err());
    }

    #[test]
    fn test_map_http_error_401() {
        assert!(matches!(
            OpenAIProvider::map_http_error(401, ""),
            StygianError::Provider(ProviderError::InvalidCredentials)
        ));
    }

    #[test]
    fn test_map_http_error_429() {
        let err = OpenAIProvider::map_http_error(429, "too many");
        assert!(
            matches!(err, StygianError::Provider(ProviderError::ApiError(ref msg)) if msg.contains("rate limited"))
        );
    }

    #[test]
    fn test_map_http_error_server_error() {
        let err = OpenAIProvider::map_http_error(500, "internal");
        assert!(
            matches!(err, StygianError::Provider(ProviderError::ApiError(ref msg)) if msg.contains("500"))
        );
    }

    #[test]
    fn test_parse_response_missing_choices() {
        let resp = serde_json::json!({"id": "chatcmpl-abc"});
        assert!(OpenAIProvider::parse_response(&resp).is_err());
    }

    #[test]
    fn test_config_with_model() {
        let cfg = OpenAIConfig::new("key".to_string()).with_model("gpt-4-turbo");
        assert_eq!(cfg.model, "gpt-4-turbo");
    }
}
