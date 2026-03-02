//! Mock AI provider adapter for testing
//!
//! A minimal implementation of AIProvider that returns predefined responses.
//! Used to validate that the port trait compiles and can be implemented.

use crate::domain::error::Result;
use crate::ports::{AIProvider, ProviderCapabilities};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use serde_json::{Value, json};

/// Mock AI provider for testing
///
/// Returns predefined JSON data without calling any actual LLM API.
/// Useful for testing pipeline execution without incurring API costs.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::mock_ai::MockAIProvider;
/// use stygian_graph::ports::AIProvider;
/// use serde_json::json;
///
/// # #[tokio::main]
/// # async fn main() {
/// let provider = MockAIProvider;
/// let schema = json!({"type": "object"});
/// let content = "Test content".to_string();
///
/// let result = provider.extract(content, schema).await.unwrap();
/// assert_eq!(result["mock"], true);
/// # }
/// ```
pub struct MockAIProvider;

#[async_trait]
impl AIProvider for MockAIProvider {
    async fn extract(&self, content: String, _schema: Value) -> Result<Value> {
        Ok(json!({
            "mock": true,
            "provider": self.name(),
            "content_length": content.len(),
            "extracted_data": {
                "title": "Mock Title",
                "description": "Mock Description",
            }
        }))
    }

    async fn stream_extract(
        &self,
        content: String,
        _schema: Value,
    ) -> Result<BoxStream<'static, Result<Value>>> {
        // Mock streaming by emitting three chunks
        let chunks = vec![
            Ok(json!({"chunk": 1, "data": "first"})),
            Ok(json!({"chunk": 2, "data": "second"})),
            Ok(json!({"chunk": 3, "data": "third", "content_length": content.len()})),
        ];
        Ok(Box::pin(stream::iter(chunks)))
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
        "mock-ai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    #[tokio::test]
    async fn test_mock_provider_extract() -> crate::domain::error::Result<()> {
        let provider = MockAIProvider;
        let schema = json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"}
            }
        });
        let content = "Test HTML content".to_string();

        let output = provider.extract(content.clone(), schema).await?;
        assert_eq!(output.get("mock").and_then(Value::as_bool), Some(true));
        assert_eq!(
            output.get("provider").and_then(Value::as_str),
            Some("mock-ai")
        );
        assert_eq!(
            output.get("content_length").and_then(Value::as_u64),
            u64::try_from(content.len()).ok()
        );
        assert_eq!(
            output
                .get("extracted_data")
                .and_then(|d| d.get("title"))
                .and_then(Value::as_str),
            Some("Mock Title")
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_mock_provider_stream_extract() -> crate::domain::error::Result<()> {
        let provider = MockAIProvider;
        let schema = json!({"type": "object"});
        let content = "Stream test content".to_string();

        let mut stream = provider.stream_extract(content.clone(), schema).await?;
        let mut chunks = Vec::new();

        while let Some(chunk_result) = stream.next().await {
            chunks.push(chunk_result?);
        }

        assert_eq!(chunks.len(), 3, "Should emit 3 chunks");
        assert_eq!(
            chunks
                .first()
                .and_then(|c| c.get("chunk"))
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            chunks
                .get(1)
                .and_then(|c| c.get("chunk"))
                .and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            chunks
                .get(2)
                .and_then(|c| c.get("chunk"))
                .and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            chunks
                .get(2)
                .and_then(|c| c.get("content_length"))
                .and_then(Value::as_u64),
            u64::try_from(content.len()).ok()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_mock_provider_capabilities() {
        let provider = MockAIProvider;
        let caps = provider.capabilities();

        assert!(caps.streaming, "Mock provider supports streaming");
        assert!(!caps.vision, "Mock provider does not support vision");
        assert!(!caps.tool_use, "Mock provider does not support tool use");
        assert!(caps.json_mode, "Mock provider supports JSON mode");
    }

    #[tokio::test]
    async fn test_mock_provider_name() {
        let provider = MockAIProvider;
        assert_eq!(provider.name(), "mock-ai");
    }

    #[tokio::test]
    async fn test_mock_provider_is_send_sync() {
        // Compile-time check that MockAIProvider implements Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockAIProvider>();
    }

    #[tokio::test]
    async fn test_default_capabilities() {
        let default_caps = ProviderCapabilities::default();
        assert!(!default_caps.streaming);
        assert!(!default_caps.vision);
        assert!(!default_caps.tool_use);
        assert!(!default_caps.json_mode);
    }

    #[tokio::test]
    async fn test_capabilities_equality() {
        let caps1 = ProviderCapabilities {
            streaming: true,
            vision: false,
            tool_use: true,
            json_mode: true,
        };
        let caps2 = ProviderCapabilities {
            streaming: true,
            vision: false,
            tool_use: true,
            json_mode: true,
        };
        assert_eq!(caps1, caps2);
    }
}
