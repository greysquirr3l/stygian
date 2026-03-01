//! Google Gemini AI provider adapter
//!
//! Implements the `AIProvider` port using Google's Generative Language API.
//! Supports Gemini 1.5 Pro and Gemini 2.0 Flash with response schema enforcement.
//!
//! # Example
//!
//! ```no_run
//! use mycelium_graph::adapters::ai::gemini::{GeminiProvider, GeminiConfig};
//! use mycelium_graph::ports::AIProvider;
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let provider = GeminiProvider::new("AIza...".to_string());
//! let schema = json!({"type": "object", "properties": {"title": {"type": "string"}}});
//! // let result = provider.extract("<html>Hello</html>".to_string(), schema).await.unwrap();
//! # });
//! ```

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use reqwest::Client;
use serde_json::{Value, json};

use crate::domain::error::{MyceliumError, ProviderError, Result};
use crate::ports::{AIProvider, ProviderCapabilities};

/// Default model
const DEFAULT_MODEL: &str = "gemini-2.0-flash";

/// Google Generative Language API base URL
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Configuration for the Gemini provider
#[derive(Debug, Clone)]
pub struct GeminiConfig {
    /// Google AI API key
    pub api_key: String,
    /// Model identifier
    pub model: String,
    /// Maximum output tokens
    pub max_tokens: u32,
    /// Request timeout
    pub timeout: Duration,
}

impl GeminiConfig {
    /// Create config with API key and defaults
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: 8192,
            timeout: Duration::from_secs(120),
        }
    }

    /// Override model
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

/// Google Gemini provider adapter
pub struct GeminiProvider {
    config: GeminiConfig,
    client: Client,
}

impl GeminiProvider {
    /// Create with API key and defaults
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::adapters::ai::gemini::GeminiProvider;
    /// let p = GeminiProvider::new("AIza...".to_string());
    /// ```
    pub fn new(api_key: String) -> Self {
        Self::with_config(GeminiConfig::new(api_key))
    }

    /// Create with custom configuration
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::adapters::ai::gemini::{GeminiProvider, GeminiConfig};
    /// let config = GeminiConfig::new("AIza...".to_string()).with_model("gemini-1.5-pro");
    /// let p = GeminiProvider::with_config(config);
    /// ```
    pub fn with_config(config: GeminiConfig) -> Self {
        // SAFETY: TLS backend (rustls) is always available; build() only fails if no TLS backend.
        #[allow(clippy::expect_used)]
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to build HTTP client");
        Self { config, client }
    }

    fn api_url(&self) -> String {
        format!(
            "{}/{}:generateContent?key={}",
            API_BASE, self.config.model, self.config.api_key
        )
    }

    fn build_body(&self, content: &str, schema: &Value) -> Value {
        let prompt = format!(
            "Extract structured data from the following content according to this JSON schema.\n\
             Return ONLY valid JSON matching the schema.\n\
             Schema: {}\n\nContent:\n{}",
            serde_json::to_string(schema).unwrap_or_default(),
            content
        );

        json!({
            "contents": [{"parts": [{"text": prompt}]}],
            "generationConfig": {
                "maxOutputTokens": self.config.max_tokens,
                "responseMimeType": "application/json",
                "responseSchema": schema
            }
        })
    }

    fn parse_response(response: &Value) -> Result<Value> {
        let text = response
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                MyceliumError::Provider(ProviderError::ApiError(
                    "No text in Gemini response".to_string(),
                ))
            })?;

        serde_json::from_str(text).map_err(|e| {
            MyceliumError::Provider(ProviderError::ApiError(format!(
                "Failed to parse Gemini JSON response: {e}"
            )))
        })
    }

    fn map_http_error(status: u16, body: &str) -> MyceliumError {
        match status {
            400 if body.contains("API_KEY") => {
                MyceliumError::Provider(ProviderError::InvalidCredentials)
            }
            429 => MyceliumError::Provider(ProviderError::ApiError(format!(
                "Gemini rate limited: {body}"
            ))),
            _ => MyceliumError::Provider(ProviderError::ApiError(format!("HTTP {status}: {body}"))),
        }
    }
}

#[async_trait]
impl AIProvider for GeminiProvider {
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
                MyceliumError::Provider(ProviderError::ApiError(format!(
                    "Gemini request failed: {e}"
                )))
            })?;

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| MyceliumError::Provider(ProviderError::ApiError(e.to_string())))?;

        if status != 200 {
            return Err(Self::map_http_error(status, &text));
        }

        let json_val: Value = serde_json::from_str(&text)
            .map_err(|e| MyceliumError::Provider(ProviderError::ApiError(e.to_string())))?;

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
            tool_use: false,
            json_mode: true,
        }
    }

    fn name(&self) -> &'static str {
        "gemini"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_name() {
        assert_eq!(GeminiProvider::new("k".to_string()).name(), "gemini");
    }

    #[test]
    fn test_capabilities() {
        let caps = GeminiProvider::new("k".to_string()).capabilities();
        assert!(caps.json_mode);
        assert!(caps.vision);
    }

    #[test]
    fn test_api_url_contains_model_and_key() {
        let p = GeminiProvider::new("my-key".to_string());
        let url = p.api_url();
        assert!(url.contains(DEFAULT_MODEL));
        assert!(url.contains("my-key"));
    }

    #[test]
    fn test_build_body_has_response_mime() {
        let p = GeminiProvider::new("k".to_string());
        let body = p.build_body("content", &json!({"type": "object"}));
        assert_eq!(
            body.get("generationConfig")
                .and_then(|gc| gc.get("responseMimeType"))
                .and_then(Value::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn test_parse_response_valid() -> Result<()> {
        let resp = json!({
            "candidates": [{
                "content": {"parts": [{"text": "{\"name\": \"Alice\"}"}]}
            }]
        });
        let val = GeminiProvider::parse_response(&resp)?;
        assert_eq!(val.get("name").and_then(Value::as_str), Some("Alice"));
        Ok(())
    }
}
