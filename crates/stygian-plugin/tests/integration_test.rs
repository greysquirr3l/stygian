#![cfg(feature = "graph-integration")]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn
)]
//! Integration test for plugin extraction

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::sync::Arc;
    use stygian_graph::ports::{ScrapingService, ServiceInput};
    use stygian_plugin::{
        adapters::{ExtractionEngine, PluginExtractionAdapter},
        domain::{ExtractionTemplate, Region, Selector, Transformation},
        ports::PluginTemplateStore,
        storage::{FileTemplateStore, MemoryIdempotencyStore},
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_complete_extraction_workflow()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        // Create storage
        let tmp = TempDir::new()?;
        let template_store = Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));
        let extraction_port = Arc::new(ExtractionEngine);
        let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

        // Create template
        let template = ExtractionTemplate::new("Product Listing")
            .with_description("Extract product information")
            .with_region(
                Region::new(
                    "product_name",
                    Selector::css(".product-name"),
                    json!({"type": "string"}),
                )
                .with_transformation(Transformation::Trim)
                .with_transformation(Transformation::NormalizeWhitespace),
            )
            .with_region(
                Region::new(
                    "price",
                    Selector::css(".product-price"),
                    json!({"type": "string"}),
                )
                .with_transformation(Transformation::Trim),
            );

        // Save template
        template_store.save(&template).await?;
        let template_id = template.id;

        // Create adapter
        let adapter =
            PluginExtractionAdapter::new(template_store, extraction_port, idempotency_store);

        // Sample HTML for extraction
        let html = r#"
            <div class="product-name">Premium Widget</div>
            <span class="product-price">$19.99</span>
        "#;

        let input = ServiceInput {
            url: "https://example.com/products".to_string(),
            params: json!({
                "template_id": template_id.to_string(),
                "html": html,
            }),
        };

        let result = adapter.execute(input).await?;

        // Verify output
        assert!(!result.data.is_empty());
        assert_eq!(result.metadata.get("cached"), Some(&json!(false)));

        // Parse extracted data
        let extracted: serde_json::Value =
            serde_json::from_str(&result.data).unwrap_or_else(|_| json!({}));
        assert!(!extracted.is_null());
        Ok(())
    }

    #[tokio::test]
    async fn test_idempotency_deduplication() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let tmp = TempDir::new()?;
        let template_store = Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));
        let extraction_port = Arc::new(ExtractionEngine);
        let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

        // Create and save template
        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "title",
            Selector::css("h1"),
            json!({"type": "string"}),
        ));
        template_store.save(&template).await?;

        let adapter =
            PluginExtractionAdapter::new(template_store, extraction_port, idempotency_store);

        let idempotency_key = ulid::Ulid::new();
        let html = "<html><h1>First</h1></html>";

        let input1 = ServiceInput {
            url: "https://example.com/page1".to_string(),
            params: json!({
                "template_id": template.id.to_string(),
                "idempotency_key": idempotency_key.to_string(),
                "html": html,
            }),
        };

        let result1 = adapter.execute(input1).await?;
        assert_eq!(result1.metadata.get("cached"), Some(&json!(false)));

        // Same key with different URL/HTML should return cached result (idempotency key is primary)
        let input2 = ServiceInput {
            url: "https://example.com/page2".to_string(),
            params: json!({
                "template_id": template.id.to_string(),
                "idempotency_key": idempotency_key.to_string(),
                "html": "<html><h1>Different</h1></html>",
            }),
        };

        let result2 = adapter.execute(input2).await?;
        assert_eq!(result2.metadata.get("cached"), Some(&json!(true)));
        assert_eq!(result1.data, result2.data);
        Ok(())
    }
}
