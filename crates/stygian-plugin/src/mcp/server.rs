//! MCP plugin server implementation
//!
//! Provides the core tool definitions and request handling for plugin extraction.

use scraper::{Html, Selector as ScraperSelector};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    ExtractionRequest, PluginError, Result,
    adapters::ExtractionEngine,
    domain::{ExtractionTemplate, Region, Selector, Transformation},
    ports::{IdempotencyKeyStore, PluginExtractionPort, PluginTemplateStore},
    storage::{FileTemplateStore, MemoryIdempotencyStore},
};

const SUPPORTED_TRANSFORMATIONS: &str = "Trim, Lowercase, Uppercase, RemoveWhitespace, NormalizeWhitespace, StripHtml, DecodeHtml, ParseJson, Regex:pattern/replacement, RegexExtract:pattern/group, Coerce:type, Filter:pattern";

/// MCP server providing plugin extraction tools
#[allow(dead_code)]
pub struct McpPluginServer {
    template_store: Arc<dyn PluginTemplateStore>,
    extraction_engine: Arc<dyn PluginExtractionPort>,
    idempotency_store: Arc<dyn IdempotencyKeyStore>,
}

impl McpPluginServer {
    /// Create a new plugin MCP server with file-based storage (development)
    pub fn new_with_file_storage(templates_dir: std::path::PathBuf) -> Self {
        Self {
            template_store: Arc::new(FileTemplateStore::new(templates_dir)),
            extraction_engine: Arc::new(ExtractionEngine),
            idempotency_store: Arc::new(MemoryIdempotencyStore::new()),
        }
    }

    /// Create with custom adapters
    pub fn with_adapters(
        template_store: Arc<dyn PluginTemplateStore>,
        extraction_engine: Arc<dyn PluginExtractionPort>,
        idempotency_store: Arc<dyn IdempotencyKeyStore>,
    ) -> Self {
        Self {
            template_store,
            extraction_engine,
            idempotency_store,
        }
    }

    fn tools_template_management() -> [Value; 3] {
        [
            json!({
                "name": "plugin_create_template",
                "description": "Create a new extraction template with the given name and optional description. Returns the template UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Template name (e.g., 'Product Listings')" },
                        "description": { "type": "string", "description": "Optional template description" },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional tags for organization"
                        }
                    },
                    "required": ["name"]
                }
            }),
            json!({
                "name": "plugin_list_templates",
                "description": "List all saved extraction templates with metadata.",
                "inputSchema": { "type": "object", "properties": {} }
            }),
            json!({
                "name": "plugin_delete_template",
                "description": "Delete an extraction template permanently.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "template_id": { "type": "string", "description": "UUID of the template to delete" }
                    },
                    "required": ["template_id"]
                }
            }),
        ]
    }

    fn tools_extraction() -> [Value; 4] {
        [
            json!({
                "name": "plugin_add_region",
                "description": "Add an extraction region (named zone) to a template. A region is a named selector with transformations.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "template_id": { "type": "string", "description": "UUID of the template" },
                        "region_name": { "type": "string", "description": "Unique name for this region (e.g., 'product_title')" },
                        "selector_css": { "type": "string", "description": "Optional CSS selector" },
                        "selector_xpath": { "type": "string", "description": "Optional XPath selector" },
                        "transformations": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Ordered transformations: 'Trim', 'Lowercase', 'Regex:pattern/replace', 'StripHtml', etc."
                        }
                    },
                    "required": ["template_id", "region_name"]
                }
            }),
            json!({
                "name": "plugin_apply_template",
                "description": "Apply an extraction template to HTML content. Returns extracted data for each region.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "template_id": { "type": "string", "description": "UUID of the template to apply" },
                        "html": { "type": "string", "description": "HTML content to extract from" },
                        "url": { "type": "string", "description": "Source URL (for logging/context)" }
                    },
                    "required": ["template_id", "html", "url"]
                }
            }),
            json!({
                "name": "plugin_get_template",
                "description": "Retrieve a template's full configuration.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "template_id": { "type": "string", "description": "UUID of the template" }
                    },
                    "required": ["template_id"]
                }
            }),
            json!({
                "name": "plugin_extract_batch",
                "description": "Apply a template to extract multiple instances from a page (e.g., all products).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "template_id": { "type": "string", "description": "UUID of the template" },
                        "html": { "type": "string", "description": "HTML content" },
                        "url": { "type": "string", "description": "Source URL" },
                        "root_selector": { "type": "string", "description": "CSS selector for parent containers to iterate over" }
                    },
                    "required": ["template_id", "html", "url", "root_selector"]
                }
            }),
        ]
    }

    fn tools_inspection() -> [Value; 1] {
        [json!({
            "name": "plugin_inspect_selector",
            "description": "Test if a CSS/XPath selector matches elements in HTML. Returns match count and preview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "html": { "type": "string", "description": "HTML to test against" },
                    "selector_css": { "type": "string", "description": "CSS selector to test" },
                    "selector_xpath": { "type": "string", "description": "XPath to test as fallback" }
                },
                "required": ["html"]
            }
        })]
    }

    /// Get the tool list for MCP protocol
    pub fn tools_list(&self) -> Vec<Value> {
        let mut tools = Vec::with_capacity(8);
        tools.extend(Self::tools_template_management());
        tools.extend(Self::tools_extraction());
        tools.extend(Self::tools_inspection());
        tools
    }

    /// Handle a tool call
    pub async fn handle_tool_call(&self, name: &str, args: &Value) -> Value {
        let result = match name {
            "plugin_create_template" => self.tool_create_template(args).await,
            "plugin_add_region" => self.tool_add_region(args).await,
            "plugin_apply_template" => self.tool_apply_template(args).await,
            "plugin_list_templates" => self.tool_list_templates(args).await,
            "plugin_delete_template" => self.tool_delete_template(args).await,
            "plugin_get_template" => self.tool_get_template(args).await,
            "plugin_extract_batch" => self.tool_extract_batch(args).await,
            "plugin_inspect_selector" => self.tool_inspect_selector(args).await,
            _ => Err(PluginError::TemplateValidationError(format!(
                "unknown tool: {name}"
            ))),
        };

        match result {
            Ok(data) => {
                json!({ "content": [{ "type": "text", "text": serde_json::to_string(&data).unwrap_or_default() }] })
            }
            Err(e) => {
                json!({ "content": [{ "type": "text", "text": format!("Error: {}", e) }], "isError": true })
            }
        }
    }

    // ── Tool implementations ───────────────────────────────────────────────

    async fn tool_create_template(&self, args: &Value) -> Result<Value> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| PluginError::TemplateValidationError("missing 'name'".to_string()))?;

        let description = args
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let tags = args
            .get("tags")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let mut template = ExtractionTemplate::new(name);
        if let Some(desc) = description {
            template = template.with_description(desc);
        }
        template = template.with_tags(tags);

        self.template_store.save(&template).await?;

        Ok(json!({
            "template_id": template.id.to_string(),
            "name": template.name,
            "created_at": template.metadata.created_at.to_rfc3339(),
        }))
    }

    async fn tool_add_region(&self, args: &Value) -> Result<Value> {
        let template_id = args
            .get("template_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                PluginError::TemplateValidationError("invalid 'template_id'".to_string())
            })?;

        let region_name = args
            .get("region_name")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                PluginError::TemplateValidationError("missing 'region_name'".to_string())
            })?;

        let selector_css = args
            .get("selector_css")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let selector_xpath = args
            .get("selector_xpath")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let selector = match (selector_css, selector_xpath) {
            (Some(css), Some(xpath)) => Selector::dual(css, xpath),
            (Some(css), None) => Selector::css(css),
            (None, Some(xpath)) => Selector::xpath(xpath),
            (None, None) => {
                return Err(PluginError::TemplateValidationError(
                    "must provide either selector_css or selector_xpath".to_string(),
                ));
            }
        };

        // Load template
        let mut template = self.template_store.get(&template_id).await?;

        // Parse transformations - validate all entries and fail on first error
        let mut transformations = Vec::new();
        if let Some(arr) = args.get("transformations").and_then(Value::as_array) {
            for (idx, v) in arr.iter().enumerate() {
                let s = v.as_str().ok_or_else(|| {
                    PluginError::TemplateValidationError(format!(
                        "transformation at index {idx} must be a string"
                    ))
                })?;
                let transformation = parse_transformation(s).map_err(|_| {
                    PluginError::TemplateValidationError(format!(
                        "invalid transformation at index {idx}: '{s}'. Supported transformations: {SUPPORTED_TRANSFORMATIONS}"
                    ))
                })?;
                transformations.push(transformation);
            }
        }

        // Create region
        let mut region = Region::new(&region_name, selector, json!({"type": "string"}));
        for t in transformations {
            region = region.with_transformation(t);
        }

        template = template.with_region(region);
        self.template_store.save(&template).await?;

        Ok(json!({
            "template_id": template.id.to_string(),
            "region_name": region_name,
            "regions_count": template.regions.len(),
        }))
    }

    async fn tool_apply_template(&self, args: &Value) -> Result<Value> {
        let template_id = args
            .get("template_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                PluginError::TemplateValidationError("invalid 'template_id'".to_string())
            })?;

        let html = args
            .get("html")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| PluginError::TemplateValidationError("missing 'html'".to_string()))?;

        let url = args
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| PluginError::TemplateValidationError("missing 'url'".to_string()))?;

        let template = self.template_store.get(&template_id).await?;
        let request = ExtractionRequest::new(template, &url, &html);
        let result = self.extraction_engine.execute(&request).await?;

        Ok(json!({
            "data": result.data,
            "metadata": {
                "regions_successful": result.metadata.region_status.values().filter(|s| s.success).count(),
                "total_regions": result.metadata.region_status.len(),
                "elapsed_ms": result.metadata.elapsed_ms,
            }
        }))
    }

    async fn tool_list_templates(&self, _args: &Value) -> Result<Value> {
        let templates = self.template_store.list().await?;
        let list: Vec<_> = templates
            .iter()
            .map(|t| {
                json!({
                    "id": t.id.to_string(),
                    "name": &t.name,
                    "description": &t.description,
                    "regions": t.regions.len(),
                    "created_at": t.metadata.created_at.to_rfc3339(),
                    "usage_count": t.metadata.usage_count,
                    "tags": &t.metadata.tags,
                })
            })
            .collect();

        Ok(json!({
            "count": list.len(),
            "templates": list,
        }))
    }

    async fn tool_delete_template(&self, args: &Value) -> Result<Value> {
        let template_id = args
            .get("template_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                PluginError::TemplateValidationError("invalid 'template_id'".to_string())
            })?;

        self.template_store.delete(&template_id).await?;

        Ok(json!({
            "deleted": template_id.to_string(),
        }))
    }

    async fn tool_get_template(&self, args: &Value) -> Result<Value> {
        let template_id = args
            .get("template_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                PluginError::TemplateValidationError("invalid 'template_id'".to_string())
            })?;

        let template = self.template_store.get(&template_id).await?;

        Ok(json!({
            "id": template.id.to_string(),
            "name": template.name,
            "description": template.description,
            "regions": template.regions.iter().map(|r| {
                json!({
                    "name": r.name,
                    "selector": format!("{:?}", r.selector),
                    "transformations": r.transformations.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "metadata": {
                "created_at": template.metadata.created_at.to_rfc3339(),
                "updated_at": template.metadata.updated_at.to_rfc3339(),
                "usage_count": template.metadata.usage_count,
            }
        }))
    }

    async fn tool_extract_batch(&self, args: &Value) -> Result<Value> {
        let template_id = args
            .get("template_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                PluginError::TemplateValidationError("invalid 'template_id'".to_string())
            })?;

        let html = args
            .get("html")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| PluginError::TemplateValidationError("missing 'html'".to_string()))?;

        let url = args
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| PluginError::TemplateValidationError("missing 'url'".to_string()))?;

        let root_selector_str = args
            .get("root_selector")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                PluginError::TemplateValidationError("missing 'root_selector'".to_string())
            })?;

        // Parse root selector as CSS (XPath not supported for batch extraction)
        let root_selector =
            ScraperSelector::parse(&root_selector_str).map_err(|_| PluginError::SelectorError {
                selector: root_selector_str.clone(),
                reason: "Failed to parse root_selector as CSS selector".to_string(),
            })?;

        // Parse HTML and find all root containers.
        // Keep this in a separate scope so non-Send scraper internals are dropped before await.
        let root_elements: Vec<String> = {
            let document = Html::parse_document(&html);
            document
                .select(&root_selector)
                .map(|elem| elem.inner_html())
                .collect()
        };

        if root_elements.is_empty() {
            return Err(PluginError::ExtractionError(format!(
                "root_selector matched no elements: {root_selector_str}"
            )));
        }

        // Extract data from each root container
        let template = self.template_store.get(&template_id).await?;
        let mut results = Vec::new();

        for root_html in root_elements {
            let request = ExtractionRequest::new(template.clone(), &url, &root_html);
            match self.extraction_engine.execute(&request).await {
                Ok(result) => {
                    results.push(json!({
                        "data": result.data,
                        "successful_regions": result.metadata.region_status.values().filter(|s| s.success).count(),
                    }));
                }
                Err(e) => {
                    // Continue with partial results on error
                    results.push(json!({
                        "error": e.to_string(),
                        "successful_regions": 0,
                    }));
                }
            }
        }

        Ok(json!({
            "root_selector": root_selector_str,
            "results": results,
            "total_matched": results.len(),
            "successful": results.iter().filter(|r| r.get("data").is_some()).count(),
        }))
    }

    async fn tool_inspect_selector(&self, args: &Value) -> Result<Value> {
        let html = args
            .get("html")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| PluginError::TemplateValidationError("missing 'html'".to_string()))?;

        let selector_css = args
            .get("selector_css")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let selector_xpath = args
            .get("selector_xpath")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let selector = match (&selector_css, &selector_xpath) {
            (Some(css), Some(xpath)) => Selector::dual(css, xpath),
            (Some(css), None) => Selector::css(css),
            (None, Some(xpath)) => Selector::xpath(xpath),
            (None, None) => {
                return Err(PluginError::TemplateValidationError(
                    "must provide either selector_css or selector_xpath".to_string(),
                ));
            }
        };

        selector.validate()?;

        // Use the CSS selector for validation/counting since XPath is not yet supported
        if let Some(css) = selector_css {
            let (is_valid, count) = self
                .extraction_engine
                .validate_selector(&html, &css)
                .await?;
            Ok(json!({
                "selector": css,
                "selector_type": "css",
                "valid": is_valid,
                "match_count": count,
                "preview": if count > 0 { "Selector matched elements" } else { "No elements matched" }
            }))
        } else if selector_xpath.is_some() {
            // XPath validation not yet implemented
            Ok(json!({
                "selector": selector_xpath,
                "selector_type": "xpath",
                "valid": true,
                "note": "XPath selectors are not yet supported for validation. Please use CSS selectors to test matches."
            }))
        } else {
            Err(PluginError::TemplateValidationError(
                "No selector provided".to_string(),
            ))
        }
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

pub(crate) fn parse_transformation(s: &str) -> Result<Transformation> {
    match s {
        "Trim" => Ok(Transformation::Trim),
        "Lowercase" => Ok(Transformation::Lowercase),
        "Uppercase" => Ok(Transformation::Uppercase),
        "RemoveWhitespace" => Ok(Transformation::RemoveWhitespace),
        "NormalizeWhitespace" => Ok(Transformation::NormalizeWhitespace),
        "StripHtml" => Ok(Transformation::StripHtml),
        "DecodeHtml" => Ok(Transformation::DecodeHtml),
        "ParseJson" => Ok(Transformation::ParseJson),
        s if s.starts_with("RegexExtract:") => s
            .strip_prefix("RegexExtract:")
            .and_then(|rest| rest.rsplit_once('/'))
            .map_or_else(
                || {
                    Err(PluginError::TemplateValidationError(
                        "RegexExtract format: RegexExtract:pattern/group".to_string(),
                    ))
                },
                |(pattern, group_str)| {
                    let group = group_str.parse::<usize>().map_err(|_| {
                        PluginError::TemplateValidationError(
                            "RegexExtract group must be a positive integer".to_string(),
                        )
                    })?;
                    Ok(Transformation::RegexExtract {
                        pattern: pattern.to_string(),
                        group,
                    })
                },
            ),
        s if s.starts_with("Coerce:") => s.strip_prefix("Coerce:").map_or_else(
            || {
                Err(PluginError::TemplateValidationError(
                    "Coerce format: Coerce:type".to_string(),
                ))
            },
            |target_type| {
                Ok(Transformation::Coerce {
                    target_type: target_type.to_string(),
                })
            },
        ),
        s if s.starts_with("Filter:") => s.strip_prefix("Filter:").map_or_else(
            || {
                Err(PluginError::TemplateValidationError(
                    "Filter format: Filter:pattern".to_string(),
                ))
            },
            |pattern| {
                Ok(Transformation::Filter {
                    pattern: pattern.to_string(),
                })
            },
        ),
        s if s.starts_with("Regex:") => s
            .strip_prefix("Regex:")
            .and_then(|rest| rest.split_once('/'))
            .map_or_else(
                || {
                    Err(PluginError::TemplateValidationError(
                        "Regex format: Regex:pattern/replacement".to_string(),
                    ))
                },
                |(pattern, replacement)| {
                    Ok(Transformation::Regex {
                        pattern: pattern.to_string(),
                        replacement: replacement.to_string(),
                    })
                },
            ),
        _ => Err(PluginError::TemplateValidationError(format!(
            "unknown transformation: {s}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_transformation() {
        assert!(parse_transformation("Trim").is_ok());
        assert!(parse_transformation("Lowercase").is_ok());
        assert!(parse_transformation("Regex:pattern/replace").is_ok());
        assert!(parse_transformation("RegexExtract:price:(\\d+\\.\\d+)/1").is_ok());
        assert!(parse_transformation("Coerce:number").is_ok());
        assert!(parse_transformation("Filter:^ok$").is_ok());
        assert!(parse_transformation("Invalid").is_err());
    }
}
