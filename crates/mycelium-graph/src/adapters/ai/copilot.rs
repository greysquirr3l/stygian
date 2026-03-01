//! GitHub Copilot AI provider adapter
//!
//! Implements the `AIProvider` port using GitHub's AI model inference gateway.
//! Authenticates with a GitHub personal access token (PAT) or GitHub App token.
//! Routes requests to GitHub's hosted AI models endpoint.
//!
//! # Example
//!
//! ```no_run
//! use mycelium_graph::adapters::ai::copilot::{CopilotProvider, CopilotConfig};
//! use mycelium_graph::ports::AIProvider;
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let provider = CopilotProvider::new("ghp_...".to_string());
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

/// GitHub Models inference endpoint
const API_URL: &str = "https://models.inference.ai.azure.com/chat/completions";

/// Default model available through GitHub Models
const DEFAULT_MODEL: &str = "gpt-4o";

/// Configuration for the GitHub Copilot provider
#[derive(Debug, Clone)]
pub struct CopilotConfig {
    /// GitHub PAT or App token with `models:read` scope
    pub token: String,
    /// Model identifier
    pub model: String,
    /// Maximum response tokens
    pub max_tokens: u32,
    /// Request timeout
    pub timeout: Duration,
}

impl CopilotConfig {
    /// Create config with token and defaults
    pub fn new(token: String) -> Self {
        Self {
            token,
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
}

/// GitHub Copilot AI provider adapter
pub struct CopilotProvider {
    config: CopilotConfig,
    client: Client,
}

impl CopilotProvider {
    /// Create with GitHub token and defaults
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::adapters::ai::copilot::CopilotProvider;
    /// let p = CopilotProvider::new("ghp_...".to_string());
    /// ```
    pub fn new(token: String) -> Self {
        Self::with_config(CopilotConfig::new(token))
    }

    /// Create with custom configuration
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::adapters::ai::copilot::{CopilotProvider, CopilotConfig};
    /// let config = CopilotConfig::new("ghp_...".to_string()).with_model("Meta-Llama-3.1-405B-Instruct");
    /// let p = CopilotProvider::with_config(config);
    /// ```
    pub fn with_config(config: CopilotConfig) -> Self {
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
                MyceliumError::Provider(ProviderError::ApiError(
                    "No content in GitHub Models response".to_string(),
                ))
            })?;

        serde_json::from_str(text).map_err(|e| {
            MyceliumError::Provider(ProviderError::ApiError(format!(
                "Failed to parse GitHub Models JSON response: {e}"
            )))
        })
    }

    fn map_http_error(status: u16, body: &str) -> MyceliumError {
        match status {
            401 | 403 => MyceliumError::Provider(ProviderError::InvalidCredentials),
            429 => MyceliumError::Provider(ProviderError::ApiError(format!(
                "GitHub Models rate limited: {body}"
            ))),
            _ => MyceliumError::Provider(ProviderError::ApiError(format!("HTTP {status}: {body}"))),
        }
    }
}

#[async_trait]
impl AIProvider for CopilotProvider {
    async fn extract(&self, content: String, schema: Value) -> Result<Value> {
        let body = self.build_body(&content, &schema);

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", &self.config.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                MyceliumError::Provider(ProviderError::ApiError(format!(
                    "GitHub Models request failed: {e}"
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
            vision: false,
            tool_use: true,
            json_mode: true,
        }
    }

    fn name(&self) -> &'static str {
        "copilot"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_name() {
        assert_eq!(CopilotProvider::new("t".to_string()).name(), "copilot");
    }

    #[test]
    fn test_capabilities_json_mode() {
        let caps = CopilotProvider::new("t".to_string()).capabilities();
        assert!(caps.json_mode);
    }

    #[test]
    fn test_build_body_json_format() {
        let p = CopilotProvider::new("t".to_string());
        let body = p.build_body("c", &json!({"type": "object"}));
        assert_eq!(
            body.get("response_format").and_then(|rf| rf.get("type")),
            Some(&json!("json_object"))
        );
    }

    #[test]
    fn test_map_http_error_403() {
        assert!(matches!(
            CopilotProvider::map_http_error(403, ""),
            MyceliumError::Provider(ProviderError::InvalidCredentials)
        ));
    }
}
