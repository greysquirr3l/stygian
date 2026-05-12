//! Extraction engine: core adapter that executes extractions against HTML
//!
//! Uses scraper (CSS selectors) to extract and transform data according to templates.

use crate::domain::{ExtractionRequest, ExtractionResult, RegionStatus};
use crate::error::PluginError;
use crate::{Result, ports::PluginExtractionPort};
use async_trait::async_trait;
use scraper::{Html, Selector as ScraperSelector};
use std::time::Instant;

/// Extraction engine: executes templates against HTML
///
/// Uses the `scraper` crate to evaluate CSS selectors against HTML,
/// applies transformations, and builds structured results.
pub struct ExtractionEngine;

impl ExtractionEngine {
    /// Execute an extraction request
    pub fn execute(request: &ExtractionRequest) -> Result<ExtractionResult> {
        request.validate()?;
        request.template.validate()?;

        let start = Instant::now();
        let document = Html::parse_document(&request.html);

        let mut result = ExtractionResult::new(request.idempotency_key);
        let mut successful_regions = 0;

        for region in &request.template.regions {
            region.validate()?;

            match execute_region(&document, region) {
                Ok(extracted_values) => {
                    let count = extracted_values.len();

                    // For single values, return as-is; for multiple, return array
                    let result_value = match extracted_values.len() {
                        0 => serde_json::json!(null),
                        1 => serde_json::Value::String(
                            extracted_values.first().cloned().ok_or_else(|| {
                                PluginError::ExtractionError(
                                    "selector matched a single value, but none were extracted"
                                        .to_string(),
                                )
                            })?,
                        ),
                        _ => serde_json::Value::Array(
                            extracted_values
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    };

                    result
                        .data
                        .insert(region.name.clone(), result_value.clone());
                    result.metadata.region_status.insert(
                        region.name.clone(),
                        RegionStatus {
                            success: true,
                            matched_count: count,
                            error: None,
                        },
                    );
                    successful_regions += 1;
                }
                Err(e) => {
                    result.metadata.region_status.insert(
                        region.name.clone(),
                        RegionStatus {
                            success: false,
                            matched_count: 0,
                            error: Some(e.to_string()),
                        },
                    );
                    result = result.with_error(format!("Region '{}': {}", region.name, e));
                }
            }
        }

        // Calculate success rate
        if request.template.regions.is_empty() {
            result.metadata.selector_success_rate = 100.0;
        } else {
            let successful = u16::try_from(successful_regions).unwrap_or(u16::MAX);
            let total = u16::try_from(request.template.regions.len()).unwrap_or(u16::MAX);
            result.metadata.selector_success_rate =
                (f32::from(successful) / f32::from(total)) * 100.0;
        }

        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        result = result.set_elapsed_ms(elapsed);

        Ok(result)
    }
}

/// Execute extraction for a single region
fn execute_region(document: &Html, region: &crate::domain::Region) -> Result<Vec<String>> {
    let selector_text = region.selector.primary();

    // Try to parse as CSS selector
    let selector =
        ScraperSelector::parse(selector_text).map_err(|e| PluginError::SelectorError {
            selector: selector_text.to_string(),
            reason: format!("Failed to parse selector: {e:?}"),
        })?;

    let mut results = Vec::new();

    // Select all matching elements
    for element in document.select(&selector) {
        let text = element.inner_html();

        // Apply transformation chain to the extracted text
        let transformed =
            crate::domain::Transformation::apply_chain(&region.transformations, text)?;

        results.push(transformed);
    }

    if results.is_empty() {
        return Err(PluginError::ExtractionError(format!(
            "No elements matched selector: {selector_text}"
        )));
    }

    Ok(results)
}

#[async_trait]
impl PluginExtractionPort for ExtractionEngine {
    async fn execute(&self, request: &ExtractionRequest) -> Result<ExtractionResult> {
        Self::execute(request)
    }

    async fn validate_selector(&self, html: &str, selector_expr: &str) -> Result<(bool, usize)> {
        let document = Html::parse_document(html);

        ScraperSelector::parse(selector_expr).map_or(Ok((false, 0)), |selector| {
            let count = document.select(&selector).count();
            Ok((true, count))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ExtractionTemplate, Region, Selector, Transformation};
    use serde_json::{Value, json};

    #[test]
    fn test_extract_single_element() -> crate::Result<()> {
        let html = r#"<div><p class="title">Hello World</p></div>"#;

        let region = Region::new("title", Selector::css(".title"), json!({"type": "string"}));
        let template = ExtractionTemplate::new("Test").with_region(region);

        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        assert!(result.is_fully_successful());
        assert_eq!(
            result.data.get("title"),
            Some(&serde_json::json!("Hello World"))
        );
        Ok(())
    }

    #[test]
    fn test_extract_multiple_elements() -> crate::Result<()> {
        let html = r#"
            <div>
                <p class="item">Item 1</p>
                <p class="item">Item 2</p>
                <p class="item">Item 3</p>
            </div>
        "#;

        let region = Region::new("items", Selector::css(".item"), json!({"type": "array"}));
        let template = ExtractionTemplate::new("Test").with_region(region);

        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        let items_len = result
            .data
            .get("items")
            .and_then(Value::as_array)
            .map(std::vec::Vec::len);
        assert_eq!(items_len, Some(3));
        Ok(())
    }

    #[test]
    fn test_extract_with_transformation() -> crate::Result<()> {
        let html = r#"<div><p class="price">  $19.99  </p></div>"#;

        let region = Region::new("price", Selector::css(".price"), json!({"type": "string"}))
            .with_transformation(Transformation::Trim);
        let template = ExtractionTemplate::new("Test").with_region(region);

        let request = ExtractionRequest::new(template, "http://example.com", html);
        let result = ExtractionEngine::execute(&request)?;

        assert_eq!(result.data.get("price"), Some(&serde_json::json!("$19.99")));
        Ok(())
    }

    #[tokio::test]
    async fn test_selector_validation() -> crate::Result<()> {
        let html = r#"<div><p class="test">Content</p></div>"#;
        let engine = ExtractionEngine;

        let (valid, count) = engine.validate_selector(html, ".test").await?;
        assert!(valid);
        assert_eq!(count, 1);

        let (valid, count) = engine.validate_selector(html, ".nonexistent").await?;
        assert!(valid);
        assert_eq!(count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_css_selector() -> crate::Result<()> {
        let html = "<div><p>Content</p></div>";
        let engine = ExtractionEngine;

        let (valid, _) = engine.validate_selector(html, ">>>invalid").await?;
        assert!(!valid);
        Ok(())
    }
}
