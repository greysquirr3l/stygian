#![cfg(feature = "graph-integration")]
#![allow(clippy::panic)]
//! Comprehensive test suite for stygian-plugin
//!
//! Tests cover:
//! - Domain types and validation
//! - Selector generation and validation
//! - Transformation pipeline
//! - Idempotency key generation
//! - Template storage and retrieval
//! - Extraction engine
//! - MCP tool handlers
//! - `ScrapingService` adapter integration

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::sync::Arc;
    use stygian_graph::ports::{ScrapingService, ServiceInput};
    use stygian_plugin::{
        ExtractionRequest, IdempotencyKey,
        adapters::{ExtractionEngine, PluginExtractionAdapter},
        domain::{ExtractionTemplate, Region, Selector, Transformation},
        mcp::McpPluginServer,
        ports::{IdempotencyKeyStore, PluginTemplateStore},
        storage::{FileTemplateStore, MemoryIdempotencyStore},
    };
    use tempfile::TempDir;

    // ─────────────────────────────────────────────────────────────────────────
    // Domain Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_region_validation() {
        // Valid region
        let region = Region::new("title", Selector::css("h1"), json!({"type": "string"}));
        assert!(region.validate().is_ok());

        // Invalid: empty name
        let mut bad_region = Region::new("", Selector::css("h1"), json!({"type": "string"}));
        bad_region.name = String::new();
        assert!(bad_region.validate().is_err());
    }

    #[test]
    fn test_template_validation() {
        // Valid template
        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "title",
            Selector::css("h1"),
            json!({"type": "string"}),
        ));
        assert!(template.validate().is_ok());

        // Invalid: empty name
        let mut bad = ExtractionTemplate::new("Test");
        bad.name = String::new();
        assert!(bad.validate().is_err());

        // Valid draft: templates can be created without regions and completed later.
        let no_regions = ExtractionTemplate::new("Test");
        assert!(no_regions.validate().is_ok());
    }

    #[test]
    fn test_selector_validation() {
        assert!(Selector::css("div.product").validate().is_ok());
        assert!(
            Selector::xpath("//div[@class='product']")
                .validate()
                .is_ok()
        );
        assert!(Selector::dual("div", "//div").validate().is_ok());

        // Empty selector should fail
        let empty = Selector::css("");
        assert!(empty.validate().is_err());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Transformation Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_transformation_trim() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let text = "  hello world  ".to_string();
        let result = Transformation::Trim.apply(&text)?;
        assert_eq!(result, "hello world");
        Ok(())
    }

    #[test]
    fn test_transformation_lowercase() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let text = "HELLO WORLD".to_string();
        let result = Transformation::Lowercase.apply(&text)?;
        assert_eq!(result, "hello world");
        Ok(())
    }

    #[test]
    fn test_transformation_chain() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let text = "  HELLO WORLD  ".to_string();
        let transformations = vec![
            Transformation::Trim,
            Transformation::Lowercase,
            Transformation::NormalizeWhitespace,
        ];

        let result = Transformation::apply_chain(&transformations, text)?;
        assert_eq!(result, "hello world");
        Ok(())
    }

    #[test]
    fn test_transformation_regex() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let text = "$99.99".to_string();
        let result = Transformation::Regex {
            pattern: r"\$(\d+\.\d{2})".to_string(),
            replacement: "$1".to_string(),
        }
        .apply(&text)?;
        assert_eq!(result, "99.99");
        Ok(())
    }

    #[test]
    fn test_transformation_coerce_number() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let text = "123.45".to_string();
        let result = Transformation::Coerce {
            target_type: "number".to_string(),
        }
        .apply(&text)?;
        assert_eq!(result, "123.45");
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Idempotency Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_idempotency_key_generation() {
        let key1 = IdempotencyKey::new();
        let key2 = IdempotencyKey::new();

        // Keys should be different
        assert_ne!(key1.inner(), key2.inner());
    }

    #[test]
    fn test_idempotency_key_parse() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let key1 = IdempotencyKey::new();
        let key_str = key1.to_string();

        let key2 = key_str.parse::<IdempotencyKey>()?;
        assert_eq!(key1.inner(), key2.inner());
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Storage Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_file_template_store_save_and_get()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "title",
            Selector::css("h1"),
            json!({"type": "string"}),
        ));
        let template_id = template.id;

        store.save(&template).await?;
        let retrieved = store.get(&template_id).await?;

        assert_eq!(retrieved.name, template.name);
        assert_eq!(retrieved.regions.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_template_store_list() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let t1 = ExtractionTemplate::new("Template1").with_region(Region::new(
            "r1",
            Selector::css("div"),
            json!({"type": "string"}),
        ));
        let t2 = ExtractionTemplate::new("Template2").with_region(Region::new(
            "r2",
            Selector::css("span"),
            json!({"type": "string"}),
        ));

        store.save(&t1).await?;
        store.save(&t2).await?;

        let templates = store.list().await?;
        assert_eq!(templates.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_file_template_store_delete() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "r",
            Selector::css("div"),
            json!({"type": "string"}),
        ));
        let template_id = template.id;

        store.save(&template).await?;
        assert!(store.get(&template_id).await?.id == template_id);

        store.delete(&template_id).await?;
        assert!(store.get(&template_id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_idempotency_store() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = MemoryIdempotencyStore::new();
        let key = IdempotencyKey::new();

        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "r",
            Selector::css("div"),
            json!({"type": "string"}),
        ));
        let request = ExtractionRequest::new(template, "http://example.com", "<div>test</div>");
        let result = ExtractionEngine::execute(&request)?;

        store.store_result(&key, &result).await?;

        let retrieved = store.get_result(&key).await?;
        assert!(retrieved.is_some());
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Extraction Engine Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_extraction_engine_basic() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let html = "<h1>Hello</h1><p>World</p>";
        let template = ExtractionTemplate::new("Test")
            .with_region(Region::new(
                "title",
                Selector::css("h1"),
                json!({"type": "string"}),
            ))
            .with_region(Region::new(
                "content",
                Selector::css("p"),
                json!({"type": "string"}),
            ));

        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        assert_eq!(result.data.len(), 2);
        assert!(result.metadata.region_status.len() >= 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_extraction_engine_with_transformations()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let html = "<span>  HELLO  </span>";
        let mut region = Region::new("text", Selector::css("span"), json!({"type": "string"}));
        region = region
            .with_transformation(Transformation::Trim)
            .with_transformation(Transformation::Lowercase);

        let template = ExtractionTemplate::new("Test").with_region(region);
        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        // Result should be trimmed and lowercased
        assert!(!result.data.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_extraction_engine_invalid_selector()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let html = "<div>test</div>";
        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "missing",
            Selector::css(".nonexistent"),
            json!({"type": "string"}),
        ));

        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        // Should still return a result, but with a failed region
        assert_eq!(
            result
                .metadata
                .region_status
                .get("missing")
                .map(|s| s.success),
            Some(false)
        );
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Adapter Integration Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_plugin_extraction_adapter() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let tmp = TempDir::new()?;
        let template_store = Arc::new(FileTemplateStore::new(tmp.path().to_path_buf()));
        let extraction_port = Arc::new(ExtractionEngine);
        let idempotency_store = Arc::new(MemoryIdempotencyStore::new());

        let template = ExtractionTemplate::new("Test").with_region(Region::new(
            "title",
            Selector::css("h1"),
            json!({"type": "string"}),
        ));
        template_store.save(&template).await?;

        let adapter =
            PluginExtractionAdapter::new(template_store, extraction_port, idempotency_store);

        let input = ServiceInput {
            url: "<html><h1>Test</h1></html>".to_string(),
            params: json!({
                "template_id": template.id.to_string(),
            }),
        };

        let result = adapter.execute(input).await?;
        assert!(!result.data.is_empty());
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MCP Server Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_mcp_tools_list() {
        let Ok(tmp) = TempDir::new() else {
            panic!("failed to create temp dir");
        };
        let server = McpPluginServer::with_adapters(
            Arc::new(FileTemplateStore::new(tmp.path().to_path_buf())),
            Arc::new(ExtractionEngine),
            Arc::new(MemoryIdempotencyStore::new()),
        );
        let tools = server.tools_list();
        assert!(!tools.is_empty());

        let tool_names: Vec<_> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();

        assert!(tool_names.contains(&"plugin_create_template"));
        assert!(tool_names.contains(&"plugin_apply_template"));
        assert!(tool_names.contains(&"plugin_list_templates"));
    }

    #[tokio::test]
    async fn test_mcp_create_template() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let server = McpPluginServer::with_adapters(
            Arc::new(FileTemplateStore::new(tmp.path().to_path_buf())),
            Arc::new(ExtractionEngine),
            Arc::new(MemoryIdempotencyStore::new()),
        );

        let response = server
            .handle_tool_call(
                "plugin_create_template",
                &json!({
                    "name": "Test Template",
                    "description": "A test template"
                }),
            )
            .await;

        assert!(response.get("content").is_some());
        assert!(response.get("isError").and_then(serde_json::Value::as_bool) != Some(true));
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // End-to-End Tests
    // ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_e2e_template_creation_and_execution()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;

        // Create template
        let mut template = ExtractionTemplate::new("E2E Test");
        let region = Region::new("title", Selector::css("h1"), json!({"type": "string"}))
            .with_transformation(Transformation::Trim);
        template = template.with_region(region);

        // Store it
        let store = FileTemplateStore::new(tmp.path().to_path_buf());
        store.save(&template).await?;
        let template_id = template.id;

        // Retrieve it
        let retrieved = store.get(&template_id).await?;
        assert_eq!(retrieved.name, "E2E Test");

        // Execute it
        let html = "<html><h1>  Hello World  </h1></html>";
        let request = ExtractionRequest::new(retrieved, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        // Verify result
        assert!(!result.data.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_e2e_multiple_regions() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let html = r#"
            <div class="product">
                <h2>Widget</h2>
                <span class="price">$99.99</span>
                <p class="description">A great widget</p>
            </div>
        "#;

        let mut template = ExtractionTemplate::new("Multi-Region");
        template = template.with_region(Region::new(
            "name",
            Selector::css("h2"),
            json!({"type": "string"}),
        ));
        template = template.with_region(
            Region::new("price", Selector::css(".price"), json!({"type": "string"}))
                .with_transformation(Transformation::Regex {
                    pattern: r"\$(.+)".to_string(),
                    replacement: "$1".to_string(),
                }),
        );
        template = template.with_region(Region::new(
            "desc",
            Selector::css(".description"),
            json!({"type": "string"}),
        ));

        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        // All three regions should be extracted
        assert!(!result.data.is_empty());
        assert!(result.is_fully_successful());
        Ok(())
    }
}
