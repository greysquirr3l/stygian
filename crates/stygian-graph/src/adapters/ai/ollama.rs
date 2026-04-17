//! Ollama (local LLM) AI provider adapter
//!
//! Implements the `AIProvider` port using Ollama's HTTP API for local inference.
//! Supports any model installed in the local Ollama instance.
//! JSON output is enforced via `format: "json"` parameter.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::ai::ollama::{OllamaProvider, OllamaConfig};
//! use stygian_graph::ports::AIProvider;
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let provider = OllamaProvider::new();
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

/// Default Ollama base URL
const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Default model for local inference
const DEFAULT_MODEL: &str = "qwen2.5:32b";

/// Configuration for the Ollama provider
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Ollama API base URL
    pub base_url: String,
    /// Model to use for inference
    pub model: String,
    /// Request timeout (may need to be long for large models)
    pub timeout: Duration,
}

impl OllamaConfig {
    /// Create config with defaults
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            timeout: Duration::from_mins(5),
        }
    }

    /// Override base URL
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override model
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Ollama local LLM provider adapter
pub struct OllamaProvider {
    config: OllamaConfig,
    client: Client,
}

impl OllamaProvider {
    /// Create with default configuration (localhost:11434, qwen2.5:32b)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::ollama::OllamaProvider;
    /// let p = OllamaProvider::new();
    /// ```
    pub fn new() -> Self {
        Self::with_config(OllamaConfig::new())
    }

    /// Create with custom configuration
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::ai::ollama::{OllamaProvider, OllamaConfig};
    /// let config = OllamaConfig::new().with_model("llama3.2:latest");
    /// let p = OllamaProvider::with_config(config);
    /// ```
    pub fn with_config(config: OllamaConfig) -> Self {
        // SAFETY: TLS backend (rustls) is always available; build() only fails if no TLS backend.
        #[allow(clippy::expect_used)]
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to build HTTP client");
        Self { config, client }
    }

    fn api_url(&self) -> String {
        format!("{}/api/generate", self.config.base_url)
    }

    fn build_body(&self, content: &str, schema: &Value) -> Value {
        let prompt = format!(
            "Extract structured data from the following content according to this JSON schema.\n\
             Return ONLY valid JSON matching the schema, with no markdown, no code blocks, no extra text.\n\
             Schema: {}\n\nContent:\n{}",
            serde_json::to_string(schema).unwrap_or_default(),
            content
        );

        json!({
            "model": self.config.model,
            "prompt": prompt,
            "stream": false,
            "format": "json"
        })
    }

    fn parse_response(response: &Value) -> Result<Value> {
        let text = response
            .get("response")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                StygianError::Provider(ProviderError::ApiError(
                    "No response field in Ollama output".to_string(),
                ))
            })?;

        serde_json::from_str(text).map_err(|e| {
            StygianError::Provider(ProviderError::ApiError(format!(
                "Failed to parse Ollama JSON response: {e}"
            )))
        })
    }

    fn map_http_error(status: u16, body: &str) -> StygianError {
        match status {
            404 => StygianError::Provider(ProviderError::ModelUnavailable(format!(
                "Model not found in Ollama: {body}"
            ))),
            _ => StygianError::Provider(ProviderError::ApiError(format!("HTTP {status}: {body}"))),
        }
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AIProvider for OllamaProvider {
    async fn extract(&self, content: String, schema: Value) -> Result<Value> {
        let body = self.build_body(&content, &schema);
        let url = self.api_url();

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                StygianError::Provider(ProviderError::ApiError(format!(
                    "Ollama request failed (is Ollama running?): {e}"
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
            vision: false,
            tool_use: false,
            json_mode: true,
        }
    }

    fn name(&self) -> &'static str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_name() {
        assert_eq!(OllamaProvider::new().name(), "ollama");
    }

    #[test]
    fn test_default() {
        let p = OllamaProvider::default();
        assert_eq!(p.config.model, DEFAULT_MODEL);
        assert_eq!(p.config.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn test_capabilities_json_mode() {
        let caps = OllamaProvider::new().capabilities();
        assert!(caps.json_mode);
        assert!(!caps.vision);
    }

    #[test]
    fn test_api_url() {
        let p = OllamaProvider::new();
        assert_eq!(p.api_url(), "http://localhost:11434/api/generate");
    }

    #[test]
    fn test_build_body_stream_false() {
        let p = OllamaProvider::new();
        let body = p.build_body("c", &json!({"type": "object"}));
        assert_eq!(body.get("stream"), Some(&json!(false)));
        assert_eq!(body.get("format").and_then(Value::as_str), Some("json"));
    }

    #[test]
    fn test_parse_response_valid() -> Result<()> {
        let resp = json!({"response": "{\"score\": 42}"});
        let val = OllamaProvider::parse_response(&resp)?;
        assert_eq!(val.get("score").and_then(Value::as_u64), Some(42));
        Ok(())
    }

    #[test]
    fn test_map_http_error_404() {
        assert!(matches!(
            OllamaProvider::map_http_error(404, "not found"),
            StygianError::Provider(ProviderError::ModelUnavailable(_))
        ));
    }

    #[test]
    fn test_config_builder() {
        let config = OllamaConfig::new()
            .with_model("llama3:latest")
            .with_base_url("http://192.168.1.10:11434");
        assert_eq!(config.model, "llama3:latest");
        assert_eq!(config.base_url, "http://192.168.1.10:11434");
    }
}
