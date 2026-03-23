//! Agent source adapter — wraps an [`AIProvider`] as a pipeline data source.
//!
//! Implements [`AgentSourcePort`] and [`ScrapingService`] so that an LLM can
//! be used as a node in the DAG pipeline.  Unlike the AI adapters (which
//! *extract* structured data from existing content), this adapter *generates*
//! content by executing a user-supplied prompt.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::agent_source::AgentSource;
//! use stygian_graph::adapters::mock_ai::MockAIProvider;
//! use stygian_graph::ports::agent_source::{AgentSourcePort, AgentRequest};
//! use serde_json::json;
//! use std::sync::Arc;
//!
//! # async fn example() {
//! let provider = Arc::new(MockAIProvider);
//! let agent = AgentSource::new(provider);
//! let resp = agent.invoke(AgentRequest {
//!     prompt: "Summarise the data".into(),
//!     context: Some("raw data here".into()),
//!     parameters: json!({}),
//! }).await.unwrap();
//! println!("{}", resp.content);
//! # }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::error::Result;
use crate::ports::agent_source::{AgentRequest, AgentResponse, AgentSourcePort};
use crate::ports::{AIProvider, ScrapingService, ServiceInput, ServiceOutput};

// ─────────────────────────────────────────────────────────────────────────────
// AgentSource
// ─────────────────────────────────────────────────────────────────────────────

/// Adapter: LLM agent as a pipeline data source.
///
/// Wraps any [`AIProvider`] and exposes it through [`AgentSourcePort`] and
/// [`ScrapingService`] for integration into DAG pipelines.
pub struct AgentSource {
    provider: Arc<dyn AIProvider>,
}

impl AgentSource {
    /// Create a new agent source backed by the given AI provider.
    ///
    /// # Arguments
    ///
    /// * `provider` - An `Arc`-wrapped [`AIProvider`] implementation.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::agent_source::AgentSource;
    /// use stygian_graph::adapters::mock_ai::MockAIProvider;
    /// use std::sync::Arc;
    ///
    /// let source = AgentSource::new(Arc::new(MockAIProvider));
    /// ```
    #[must_use]
    pub fn new(provider: Arc<dyn AIProvider>) -> Self {
        Self { provider }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AgentSourcePort
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl AgentSourcePort for AgentSource {
    async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
        // Build a combined prompt+context string for the AI provider
        let content = match &request.context {
            Some(ctx) => format!("{}\n\n---\n\n{ctx}", request.prompt),
            None => request.prompt.clone(),
        };

        // Use the provider's extract method with the parameters as schema
        // (the provider returns JSON matching the "schema", which here is the
        // caller's parameters object — giving the provider guidance on what to
        // generate).
        let schema = if request.parameters.is_null()
            || request.parameters.is_object()
                && request.parameters.as_object().is_some_and(|m| m.is_empty())
        {
            json!({"type": "object", "properties": {"response": {"type": "string"}}})
        } else {
            request.parameters.clone()
        };

        let result = self.provider.extract(content, schema).await?;

        // Extract a textual response from the provider's output
        let content_text = if let Some(s) = result.get("response").and_then(Value::as_str) {
            s.to_string()
        } else {
            serde_json::to_string(&result).unwrap_or_default()
        };

        Ok(AgentResponse {
            content: content_text,
            metadata: json!({
                "provider": self.provider.name(),
                "raw_output": result,
            }),
        })
    }

    fn source_name(&self) -> &str {
        "agent"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScrapingService (DAG integration)
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for AgentSource {
    /// Invoke the LLM agent with prompt data from the pipeline.
    ///
    /// Expected params:
    /// ```json
    /// { "prompt": "Summarise this page", "parameters": {} }
    /// ```
    ///
    /// The `input.url` field is ignored; the prompt and optional upstream data
    /// are passed via `params`.
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let prompt = input.params["prompt"]
            .as_str()
            .unwrap_or("Process the following data")
            .to_string();

        let context = input.params["context"].as_str().map(String::from);
        let parameters = input.params.get("parameters").cloned().unwrap_or(json!({}));

        let request = AgentRequest {
            prompt,
            context,
            parameters,
        };

        let response = self.invoke(request).await?;

        Ok(ServiceOutput {
            data: response.content,
            metadata: response.metadata,
        })
    }

    fn name(&self) -> &'static str {
        "agent"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::mock_ai::MockAIProvider;

    fn make_agent() -> AgentSource {
        AgentSource::new(Arc::new(MockAIProvider))
    }

    #[tokio::test]
    async fn invoke_returns_response() {
        let agent = make_agent();
        let req = AgentRequest {
            prompt: "Say hello".into(),
            context: None,
            parameters: json!({}),
        };
        let resp = agent.invoke(req).await.unwrap();
        // MockAIProvider returns {"mock": true, ...} so content will be the
        // JSON serialisation of the full output (no "response" key).
        assert!(!resp.content.is_empty());
        assert_eq!(resp.metadata["provider"].as_str(), Some("mock-ai"),);
    }

    #[tokio::test]
    async fn invoke_with_context() {
        let agent = make_agent();
        let req = AgentRequest {
            prompt: "Summarise".into(),
            context: Some("some article text".into()),
            parameters: json!({}),
        };
        let resp = agent.invoke(req).await.unwrap();
        assert!(!resp.content.is_empty());
    }

    #[tokio::test]
    async fn scraping_service_execute() {
        let agent = make_agent();
        let input = ServiceInput {
            url: String::new(),
            params: json!({
                "prompt": "Generate a summary",
            }),
        };
        let output = agent.execute(input).await.unwrap();
        assert!(!output.data.is_empty());
        assert_eq!(output.metadata["provider"].as_str(), Some("mock-ai"));
    }

    #[test]
    fn source_name() {
        let agent = make_agent();
        assert_eq!(AgentSourcePort::source_name(&agent), "agent");
    }

    #[test]
    fn service_name() {
        let agent = make_agent();
        assert_eq!(ScrapingService::name(&agent), "agent");
    }
}
