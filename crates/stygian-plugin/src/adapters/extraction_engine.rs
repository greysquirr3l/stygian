//! Extraction engine: core adapter that executes extractions against HTML
//!
//! Uses scraper (CSS selectors) to extract and transform data according to templates.

use crate::domain::{ExtractionRequest, ExtractionResult, RegionStatus};
use crate::error::PluginError;
use crate::{Result, ports::PluginExtractionPort};
use async_trait::async_trait;
use scraper::{Html, Selector as ScraperSelector};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;

/// Extraction engine: executes templates against HTML
///
/// Uses the `scraper` crate to evaluate CSS selectors against HTML,
/// applies transformations, and builds structured results.
pub struct ExtractionEngine;

#[derive(Debug, Clone, Serialize)]
pub struct TransformationDebugStep {
    pub transformation: String,
    pub input: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionDebugInfo {
    pub selector: String,
    pub selector_kind: String,
    pub evaluation_scope: String,
    pub match_count: usize,
    pub raw_match_html: Option<String>,
    pub raw_extracted_value: Option<String>,
    pub transformation_output_chain: Vec<TransformationDebugStep>,
    pub final_value: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractionDebugInfo {
    pub evaluation_scope: String,
    pub root_html_snippet: String,
    pub regions: HashMap<String, RegionDebugInfo>,
}

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
                    let result_value = if count == 1 {
                        serde_json::Value::String(extracted_values.into_iter().next().ok_or_else(
                            || {
                                PluginError::ExtractionError(
                                    "selector matched a single value, but none were extracted"
                                        .to_string(),
                                )
                            },
                        )?)
                    } else {
                        serde_json::Value::Array(
                            extracted_values
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        )
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

    pub fn diagnose(request: &ExtractionRequest, evaluation_scope: &str) -> ExtractionDebugInfo {
        let document = Html::parse_document(&request.html);
        let mut regions = HashMap::new();

        for region in &request.template.regions {
            regions.insert(
                region.name.clone(),
                diagnose_region(&document, region, evaluation_scope),
            );
        }

        ExtractionDebugInfo {
            evaluation_scope: evaluation_scope.to_string(),
            root_html_snippet: truncate_debug(&request.html, 2_000),
            regions,
        }
    }
}

/// Execute extraction for a single region
fn execute_region(document: &Html, region: &crate::domain::Region) -> Result<Vec<String>> {
    // Check selector type and route accordingly
    let selector_text = match &region.selector {
        crate::domain::Selector::XPath(_) => {
            return Err(crate::error::PluginError::ExtractionError(
                "XPath selectors are not yet supported. Please use CSS selectors instead."
                    .to_string(),
            ));
        }
        crate::domain::Selector::Css(css) | crate::domain::Selector::Both { css, .. } => css,
    };

    // Parse as CSS selector
    let selector = ScraperSelector::parse(selector_text).map_err(|e| {
        crate::error::PluginError::SelectorError {
            selector: selector_text.clone(),
            reason: format!("Failed to parse CSS selector: {e:?}"),
        }
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
        return Err(crate::error::PluginError::ExtractionError(format!(
            "No elements matched CSS selector: {selector_text}"
        )));
    }

    Ok(results)
}

fn diagnose_region(
    document: &Html,
    region: &crate::domain::Region,
    evaluation_scope: &str,
) -> RegionDebugInfo {
    let (selector_kind, selector_text) = match &region.selector {
        crate::domain::Selector::Css(css) => ("css", css.as_str()),
        crate::domain::Selector::XPath(xpath) => ("xpath", xpath.as_str()),
        crate::domain::Selector::Both { css, .. } => ("dual", css.as_str()),
    };

    if matches!(&region.selector, crate::domain::Selector::XPath(_)) {
        return RegionDebugInfo {
            selector: selector_text.to_string(),
            selector_kind: selector_kind.to_string(),
            evaluation_scope: evaluation_scope.to_string(),
            match_count: 0,
            raw_match_html: None,
            raw_extracted_value: None,
            transformation_output_chain: Vec::new(),
            final_value: None,
            error: Some(
                "XPath selectors are not yet supported. Please use CSS selectors instead."
                    .to_string(),
            ),
        };
    }

    let selector = match ScraperSelector::parse(selector_text) {
        Ok(selector) => selector,
        Err(error) => {
            return RegionDebugInfo {
                selector: selector_text.to_string(),
                selector_kind: selector_kind.to_string(),
                evaluation_scope: evaluation_scope.to_string(),
                match_count: 0,
                raw_match_html: None,
                raw_extracted_value: None,
                transformation_output_chain: Vec::new(),
                final_value: None,
                error: Some(format!("Failed to parse CSS selector: {error:?}")),
            };
        }
    };

    let elements: Vec<_> = document.select(&selector).collect();
    let match_count = elements.len();

    let Some(first_match) = elements.first() else {
        return RegionDebugInfo {
            selector: selector_text.to_string(),
            selector_kind: selector_kind.to_string(),
            evaluation_scope: evaluation_scope.to_string(),
            match_count,
            raw_match_html: None,
            raw_extracted_value: None,
            transformation_output_chain: Vec::new(),
            final_value: None,
            error: Some(format!("No elements matched CSS selector: {selector_text}")),
        };
    };

    let raw_match_html = truncate_debug(&first_match.html(), 800);
    let raw_extracted_value = first_match.inner_html();
    let (transformation_output_chain, final_value, error) =
        trace_transformations(&region.transformations, &raw_extracted_value);

    RegionDebugInfo {
        selector: selector_text.to_string(),
        selector_kind: selector_kind.to_string(),
        evaluation_scope: evaluation_scope.to_string(),
        match_count,
        raw_match_html: Some(raw_match_html),
        raw_extracted_value: Some(truncate_debug(&raw_extracted_value, 800)),
        transformation_output_chain,
        final_value: final_value.map(|value| truncate_debug(&value, 800)),
        error,
    }
}

fn trace_transformations(
    transformations: &[crate::domain::Transformation],
    raw_value: &str,
) -> (Vec<TransformationDebugStep>, Option<String>, Option<String>) {
    let mut current = raw_value.to_string();
    let mut steps = Vec::with_capacity(transformations.len());

    for transformation in transformations {
        let input = current.clone();
        match transformation.apply(&current) {
            Ok(output) => {
                steps.push(TransformationDebugStep {
                    transformation: format!("{transformation:?}"),
                    input: truncate_debug(&input, 400),
                    output: Some(truncate_debug(&output, 400)),
                    error: None,
                });
                current = output;
            }
            Err(error) => {
                let error_text = error.to_string();
                steps.push(TransformationDebugStep {
                    transformation: format!("{transformation:?}"),
                    input: truncate_debug(&input, 400),
                    output: None,
                    error: Some(error_text.clone()),
                });
                return (steps, None, Some(error_text));
            }
        }
    }

    (steps, Some(current), None)
}

fn truncate_debug(value: &str, max_chars: usize) -> String {
    let mut truncated = String::new();

    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }

    truncated
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

    #[tokio::test]
    async fn test_supported_css_selector_features() -> crate::Result<()> {
        let html = r#"
            <table>
                <tr data-testid="person-row">
                    <td>skip</td>
                    <td><span class="name">Ada Lovelace</span></td>
                    <td data-testid="name-cell"><span class="title">Founder</span></td>
                </tr>
            </table>
        "#;
        let engine = ExtractionEngine;

        let (valid, count) = engine
            .validate_selector(
                html,
                "td:nth-child(2), [data-testid*='name'] .title, tr[data-testid='person-row'] .name",
            )
            .await?;

        assert!(valid);
        assert_eq!(count, 3);
        Ok(())
    }

    #[test]
    fn test_diagnostics_capture_match_and_transformations() {
        let html = r#"<div><span class="name">  Ada Lovelace  </span></div>"#;
        let region = Region::new(
            "full_name",
            Selector::css(".name"),
            json!({"type": "string"}),
        )
        .with_transformation(Transformation::Trim)
        .with_transformation(Transformation::Uppercase);
        let template = ExtractionTemplate::new("Debug Test").with_region(region);
        let request = ExtractionRequest::new(template, "http://example.com", html);

        let diagnostics = ExtractionEngine::diagnose(&request, "document");
        let region = diagnostics.regions.get("full_name");

        assert!(region.is_some());
        assert_eq!(region.map(|value| value.match_count), Some(1));
        assert_eq!(
            region.and_then(|value| value.final_value.as_deref()),
            Some("ADA LOVELACE"),
        );
        assert_eq!(
            region.map(|value| value.transformation_output_chain.len()),
            Some(2),
        );
        assert!(
            region
                .and_then(|value| value.raw_match_html.as_deref())
                .is_some_and(|value| value.contains("Ada Lovelace"))
        );
    }
}
