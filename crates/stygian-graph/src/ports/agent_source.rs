//! Agent source port trait for LLM-as-data-source.
//!
//! Defines the interface for using an AI agent (LLM) as a data source
//! within the pipeline.  Unlike [`AIProvider`](crate::ports::AIProvider),
//! which extracts structured data from existing content, an agent source
//! *generates* content by executing a prompt — making it suitable for
//! summarisation, enrichment, or synthetic data generation steps in a DAG.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::error::Result;

/// Configuration for an agent source invocation.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::agent_source::AgentRequest;
/// use serde_json::json;
///
/// let req = AgentRequest {
///     prompt: "Summarise this article".into(),
///     context: Some("The article text goes here...".into()),
///     parameters: json!({"temperature": 0.3}),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// The prompt / instruction for the agent.
    pub prompt: String,
    /// Optional context to feed alongside the prompt (e.g. scraped content
    /// from an upstream pipeline node).
    pub context: Option<String>,
    /// Provider-specific parameters (`temperature`, `max_tokens`, etc.).
    pub parameters: Value,
}

/// Response from an agent source invocation.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::agent_source::AgentResponse;
/// use serde_json::json;
///
/// let resp = AgentResponse {
///     content: "Here is a concise summary…".into(),
///     metadata: json!({"tokens_used": 142}),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Generated content from the agent.
    pub content: String,
    /// Provider-specific metadata (token counts, model info, etc.).
    pub metadata: Value,
}

/// Port trait for LLM agent data sources.
///
/// Implementations wrap an LLM provider and expose it as a pipeline-compatible
/// data source.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::ports::agent_source::{AgentSourcePort, AgentRequest};
/// use serde_json::json;
///
/// # async fn example(agent: impl AgentSourcePort) {
/// let req = AgentRequest {
///     prompt: "List the key takeaways".into(),
///     context: Some("...article text...".into()),
///     parameters: json!({}),
/// };
/// let resp = agent.invoke(req).await.unwrap();
/// println!("{}", resp.content);
/// # }
/// ```
#[async_trait]
pub trait AgentSourcePort: Send + Sync {
    /// Invoke the agent with the given request.
    ///
    /// # Arguments
    ///
    /// * `request` - Prompt, optional context, and parameters.
    ///
    /// # Returns
    ///
    /// * `Ok(AgentResponse)` - Generated content and metadata.
    /// * `Err(StygianError)` - Provider error, rate limit, etc.
    async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse>;

    /// Name of this agent source for logging and identification.
    fn source_name(&self) -> &str;
}
