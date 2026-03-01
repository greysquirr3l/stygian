//! No-op service adapter for testing
//!
//! A minimal implementation of ScrapingService that does nothing but return
//! success. Used to validate that the port trait compiles and can be implemented.

use crate::domain::error::Result;
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
use async_trait::async_trait;
use serde_json::json;

/// No-operation scraping service
///
/// Returns empty data with success metadata. Useful for testing pipeline
/// execution without performing actual I/O operations.
///
/// # Example
///
/// ```
/// use mycelium_graph::adapters::noop::NoopService;
/// use mycelium_graph::ports::{ScrapingService, ServiceInput};
/// use serde_json::json;
///
/// # #[tokio::main]
/// # async fn main() {
/// let service = NoopService;
/// let input = ServiceInput {
///     url: "https://example.com".to_string(),
///     params: json!({}),
/// };
///
/// let output = service.execute(input).await.unwrap();
/// assert_eq!(output.data, "");
/// assert_eq!(output.metadata["success"], true);
/// # }
/// ```
pub struct NoopService;

#[async_trait]
impl ScrapingService for NoopService {
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        Ok(ServiceOutput {
            data: String::new(),
            metadata: json!({
                "success": true,
                "url": input.url,
                "service": self.name(),
            }),
        })
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_noop_service_executes() -> Result<()> {
        let service = NoopService;
        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({"test": true}),
        };

        let output = service.execute(input).await?;
        assert_eq!(output.data, "", "NoopService returns empty data");
        assert_eq!(
            output.metadata.get("success"),
            Some(&serde_json::json!(true)),
            "Metadata should indicate success"
        );
        assert_eq!(
            output.metadata.get("url"),
            Some(&serde_json::json!("https://example.com")),
            "Metadata should include URL"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_noop_service_name() {
        let service = NoopService;
        assert_eq!(service.name(), "noop");
    }

    #[tokio::test]
    async fn test_noop_service_is_send_sync() {
        // Compile-time check that NoopService implements Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoopService>();
    }

    #[tokio::test]
    async fn test_multiple_executions() {
        let service = NoopService;

        // Execute multiple times to ensure stateless behavior
        for i in 0..5 {
            let url = format!("https://example.com/page{i}");
            let input = ServiceInput {
                url,
                params: json!({"iteration": i}),
            };

            let result = service.execute(input).await;
            assert!(result.is_ok(), "All executions should succeed");
        }
    }
}
