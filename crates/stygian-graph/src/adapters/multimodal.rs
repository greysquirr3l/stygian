//! Multi-modal content extraction adapter
//!
//! Routes non-HTML content (PDFs, images, CSV, JSON, XML) through appropriate
//! parsers or AI vision providers depending on content type.
//!
//! ## Content Routing
//!
//! | Content type          | Strategy                                    |
//! |-----------------------|---------------------------------------------|
//! | `text/csv`            | Parse in-process via CSV iterator           |
//! | `application/json`    | Parse + re-format via serde_json            |
//! | `text/xml` / `application/xml` | Lightweight attribute extraction   |
//! | `image/*`             | Delegate to vision-capable `AIProvider`     |
//! | `application/pdf`     | Text extraction (requires `pdf` feature)    |
//! | Unknown               | Return raw bytes as UTF-8 string            |
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::multimodal::{MultiModalAdapter, MultiModalConfig};
//! use stygian_graph::ports::{ScrapingService, ServiceInput};
//! use serde_json::json;
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let adapter = MultiModalAdapter::new(MultiModalConfig::default(), None);
//! let input = ServiceInput {
//!     url: "data:text/csv,name,age\nalice,30\nbob,25".to_string(),
//!     params: json!({ "content_type": "text/csv" }),
//! };
//! // let output = adapter.execute(input).await.unwrap();
//! # });
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::error::{StygianError, ProviderError, Result, ServiceError};
use crate::ports::{AIProvider, ScrapingService, ServiceInput, ServiceOutput};

/// Detected or declared content type for multi-modal routing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentType {
    /// Comma-separated values
    Csv,
    /// JSON text
    Json,
    /// XML / HTML-like markup
    Xml,
    /// Image (JPEG, PNG, GIF, WebP, etc.)
    Image(String),
    /// PDF document
    Pdf,
    /// Unknown / pass-through
    Unknown(String),
}

impl ContentType {
    /// Detect content type from a MIME type string or file extension
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    pub fn detect(mime_or_ext: &str) -> Self {
        let lower = mime_or_ext.to_lowercase();
        if lower.contains("csv") || lower.ends_with(".csv") {
            Self::Csv
        } else if lower.contains("json") || lower.ends_with(".json") {
            Self::Json
        } else if lower.contains("xml") || lower.ends_with(".xml") || lower.ends_with(".html") {
            Self::Xml
        } else if lower.contains("image/")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".png")
            || lower.ends_with(".gif")
            || lower.ends_with(".webp")
        {
            Self::Image(lower)
        } else if lower.contains("pdf") || lower.ends_with(".pdf") {
            Self::Pdf
        } else {
            Self::Unknown(lower)
        }
    }
}

/// Configuration for multi-modal extraction
#[derive(Debug, Clone)]
pub struct MultiModalConfig {
    /// Maximum bytes of CSV to parse (rows beyond this are dropped)
    pub max_csv_rows: usize,
    /// JSON schema to pass to the vision provider for image extraction
    pub default_image_schema: Value,
    /// Whether to attempt PDF text extraction (requires external tooling)
    pub pdf_enabled: bool,
}

impl Default for MultiModalConfig {
    fn default() -> Self {
        Self {
            max_csv_rows: 10_000,
            default_image_schema: json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string"},
                    "text_content": {"type": "string"},
                    "objects": {"type": "array", "items": {"type": "string"}}
                }
            }),
            pdf_enabled: false,
        }
    }
}

/// Multi-modal content extraction adapter
///
/// Implements `ScrapingService` by routing content to the appropriate parser
/// based on the declared `content_type` parameter.
///
/// An optional `AIProvider` (vision-capable) can be injected for image analysis.
pub struct MultiModalAdapter {
    config: MultiModalConfig,
    /// Optional vision-capable AI provider for image understanding
    vision_provider: Option<Arc<dyn AIProvider>>,
}

impl MultiModalAdapter {
    /// Create a new multi-modal adapter
    ///
    /// # Arguments
    ///
    /// * `config` - Extraction configuration
    /// * `vision_provider` - Optional vision-capable AI provider (e.g. Claude, GPT-4o)
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::multimodal::{MultiModalAdapter, MultiModalConfig};
    ///
    /// let adapter = MultiModalAdapter::new(MultiModalConfig::default(), None);
    /// ```
    pub fn new(config: MultiModalConfig, vision_provider: Option<Arc<dyn AIProvider>>) -> Self {
        Self {
            config,
            vision_provider,
        }
    }

    /// Parse CSV text into a JSON array of row objects
    #[allow(clippy::unnecessary_wraps)]
    fn parse_csv(&self, data: &str) -> Result<Value> {
        let mut lines = data.lines();
        let headers: Vec<&str> = match lines.next() {
            Some(h) => h.split(',').map(str::trim).collect(),
            None => {
                return Ok(json!({"rows": [], "row_count": 0}));
            }
        };

        let mut rows = Vec::new();
        for (i, line) in lines.enumerate() {
            if i >= self.config.max_csv_rows {
                break;
            }
            let values: Vec<&str> = line.split(',').map(str::trim).collect();
            let mut obj = serde_json::Map::new();
            for (header, val) in headers.iter().zip(values.iter()) {
                // Attempt numeric coercion, fall back to string
                if let Ok(n) = val.parse::<f64>() {
                    obj.insert((*header).to_string(), json!(n));
                } else {
                    obj.insert((*header).to_string(), json!(*val));
                }
            }
            rows.push(Value::Object(obj));
        }

        let row_count = rows.len();
        Ok(json!({
            "rows": rows,
            "row_count": row_count,
            "columns": headers
        }))
    }

    /// Validate + re-emit JSON (normalises formatting)
    fn parse_json(data: &str) -> Result<Value> {
        serde_json::from_str(data).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "Failed to parse JSON content: {e}"
            )))
        })
    }

    /// Extract text/attribute data from XML without external crates.
    ///
    /// Uses a simple regex-free approach: strips XML/HTML tags and returns the
    /// inner text content. A production implementation would use quick-xml.
    fn parse_xml(data: &str) -> Value {
        // Strip XML/HTML tags — good enough for text extraction
        let mut text = String::with_capacity(data.len());
        let mut in_tag = false;
        for ch in data.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                c if !in_tag => text.push(c),
                _ => {}
            }
        }

        // Collapse whitespace
        let cleaned: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        json!({
            "text_content": cleaned,
            "raw_length": data.len()
        })
    }

    /// Dispatch image data to a vision AI provider if one is configured
    async fn extract_image(&self, data: &str, schema: &Value) -> Result<Value> {
        match &self.vision_provider {
            Some(provider) => {
                if !provider.capabilities().vision {
                    return Err(StygianError::Provider(ProviderError::ApiError(format!(
                        "Configured vision provider '{}' does not support vision",
                        provider.name()
                    ))));
                }
                provider.extract(data.to_string(), schema.clone()).await
            }
            None => {
                // No vision provider — return metadata placeholder
                Ok(json!({
                    "status": "no_vision_provider",
                    "message": "Inject a vision-capable AIProvider to enable image understanding",
                    "data_length": data.len()
                }))
            }
        }
    }

    /// PDF text extraction (currently a stub pending the `pdf` feature)
    fn extract_pdf(data: &str, enabled: bool) -> Value {
        if enabled {
            // Future: integrate pdf-extract or pdfium-render crate
            json!({
                "status": "pdf_extraction_stub",
                "message": "PDF text extraction requires the 'pdf' feature flag",
                "data_length": data.len()
            })
        } else {
            json!({
                "status": "pdf_disabled",
                "message": "PDF extraction is disabled. Set MultiModalConfig::pdf_enabled = true",
                "data_length": data.len()
            })
        }
    }
}

#[async_trait]
impl ScrapingService for MultiModalAdapter {
    /// Extract structured content from multi-modal input
    ///
    /// Reads `content_type` from `params` (or falls back to `"unknown"`).
    /// The actual content must be in `params["content"]` or `input.url`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::multimodal::{MultiModalAdapter, MultiModalConfig};
    /// use stygian_graph::ports::{ScrapingService, ServiceInput};
    /// use serde_json::json;
    ///
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let adapter = MultiModalAdapter::new(MultiModalConfig::default(), None);
    /// let input = ServiceInput {
    ///     url: "name,age\nalice,30".to_string(),
    ///     params: json!({ "content_type": "text/csv" }),
    /// };
    /// // let output = adapter.execute(input).await.unwrap();
    /// # });
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let mime = input
            .params
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        let content = input
            .params
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or(&input.url);

        let content_type = ContentType::detect(mime);

        let (extracted, type_name) = match &content_type {
            ContentType::Csv => (self.parse_csv(content)?, "csv"),
            ContentType::Json => (Self::parse_json(content)?, "json"),
            ContentType::Xml => (Self::parse_xml(content), "xml"),
            ContentType::Image(_) => {
                let schema = input
                    .params
                    .get("schema")
                    .cloned()
                    .unwrap_or_else(|| self.config.default_image_schema.clone());
                (self.extract_image(content, &schema).await?, "image")
            }
            ContentType::Pdf => (Self::extract_pdf(content, self.config.pdf_enabled), "pdf"),
            ContentType::Unknown(_) => (json!({"raw": content}), "unknown"),
        };

        Ok(ServiceOutput {
            data: extracted.to_string(),
            metadata: json!({
                "content_type": mime,
                "detected_type": type_name,
                "input_length": content.len(),
            }),
        })
    }

    fn name(&self) -> &'static str {
        "multimodal"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn adapter() -> MultiModalAdapter {
        MultiModalAdapter::new(MultiModalConfig::default(), None)
    }

    fn input(content_type: &str, data: &str) -> ServiceInput {
        ServiceInput {
            url: data.to_string(),
            params: json!({ "content_type": content_type }),
        }
    }

    #[test]
    fn test_name() {
        assert_eq!(adapter().name(), "multimodal");
    }

    // --- ContentType detection ---

    #[test]
    fn test_detect_csv() {
        assert_eq!(ContentType::detect("text/csv"), ContentType::Csv);
        assert_eq!(ContentType::detect("file.csv"), ContentType::Csv);
    }

    #[test]
    fn test_detect_json() {
        assert_eq!(ContentType::detect("application/json"), ContentType::Json);
    }

    #[test]
    fn test_detect_xml() {
        assert_eq!(ContentType::detect("text/xml"), ContentType::Xml);
    }

    #[test]
    fn test_detect_image() {
        assert!(matches!(
            ContentType::detect("image/png"),
            ContentType::Image(_)
        ));
        assert!(matches!(
            ContentType::detect("photo.jpg"),
            ContentType::Image(_)
        ));
    }

    #[test]
    fn test_detect_pdf() {
        assert_eq!(ContentType::detect("application/pdf"), ContentType::Pdf);
    }

    // --- CSV parsing ---

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_parse_csv_basic() -> crate::domain::error::Result<()> {
        let a = adapter();
        let result = a.parse_csv("name,age\nalice,30\nbob,25")?;
        assert_eq!(result.get("row_count").and_then(Value::as_u64), Some(2));
        assert_eq!(
            result
                .get("rows")
                .and_then(|r| r.get(0))
                .and_then(|row| row.get("name"))
                .and_then(Value::as_str),
            Some("alice")
        );
        assert_eq!(
            result
                .get("rows")
                .and_then(|r| r.get(0))
                .and_then(|row| row.get("age"))
                .and_then(Value::as_f64),
            Some(30.0)
        );
        Ok(())
    }

    #[test]
    fn test_parse_csv_empty() -> crate::domain::error::Result<()> {
        let a = adapter();
        let result = a.parse_csv("")?;
        assert_eq!(result.get("row_count").and_then(Value::as_u64), Some(0));
        Ok(())
    }

    #[test]
    fn test_parse_csv_headers_only() -> crate::domain::error::Result<()> {
        let a = adapter();
        let result = a.parse_csv("col1,col2")?;
        assert_eq!(result.get("row_count").and_then(Value::as_u64), Some(0));
        Ok(())
    }

    // --- JSON parsing ---

    #[test]
    fn test_parse_json_valid() -> crate::domain::error::Result<()> {
        let result = MultiModalAdapter::parse_json(r#"{"hello": "world"}"#)?;
        assert_eq!(result.get("hello").and_then(Value::as_str), Some("world"));
        Ok(())
    }

    #[test]
    fn test_parse_json_invalid() {
        assert!(MultiModalAdapter::parse_json("not json").is_err());
    }

    // --- XML parsing ---

    #[test]
    fn test_parse_xml_strips_tags() {
        let result = MultiModalAdapter::parse_xml("<root><name>Alice</name></root>");
        let text = result
            .get("text_content")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(text.contains("Alice"));
        assert!(!text.contains('<'));
    }

    // --- PDF ---

    #[test]
    fn test_pdf_disabled_returns_status() {
        let result = MultiModalAdapter::extract_pdf("data", false);
        assert_eq!(
            result.get("status").and_then(Value::as_str),
            Some("pdf_disabled")
        );
    }

    // --- execute() integration ---

    #[tokio::test]
    async fn test_execute_csv() -> crate::domain::error::Result<()> {
        let a = adapter();
        let output = a.execute(input("text/csv", "x,y\n1,2")).await?;
        let data: Value = serde_json::from_str(&output.data)
            .map_err(|e| ServiceError::InvalidResponse(e.to_string()))?;
        assert_eq!(data.get("row_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            output.metadata.get("detected_type").and_then(Value::as_str),
            Some("csv")
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_json() -> crate::domain::error::Result<()> {
        let a = adapter();
        let out = a
            .execute(input("application/json", r#"{"k": "v"}"#))
            .await?;
        let data: Value = serde_json::from_str(&out.data)
            .map_err(|e| ServiceError::InvalidResponse(e.to_string()))?;
        assert_eq!(data.get("k").and_then(Value::as_str), Some("v"));
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_image_no_provider() -> crate::domain::error::Result<()> {
        let a = adapter();
        let out = a.execute(input("image/png", "binary-data")).await?;
        let data: Value = serde_json::from_str(&out.data)
            .map_err(|e| ServiceError::InvalidResponse(e.to_string()))?;
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("no_vision_provider")
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_unknown_passthrough() -> crate::domain::error::Result<()> {
        let a = adapter();
        let out = a.execute(input("application/octet-stream", "raw")).await?;
        let data: Value = serde_json::from_str(&out.data)
            .map_err(|e| ServiceError::InvalidResponse(e.to_string()))?;
        assert_eq!(data.get("raw").and_then(Value::as_str), Some("raw"));
        Ok(())
    }

    #[tokio::test]
    async fn test_content_from_params_overrides_url() -> crate::domain::error::Result<()> {
        let a = adapter();
        let input = ServiceInput {
            url: "should-not-be-used".to_string(),
            params: json!({
                "content_type": "application/json",
                "content": "{\"answer\": 42}"
            }),
        };
        let out = a.execute(input).await?;
        let data: Value = serde_json::from_str(&out.data)
            .map_err(|e| ServiceError::InvalidResponse(e.to_string()))?;
        assert_eq!(data.get("answer").and_then(Value::as_u64), Some(42));
        Ok(())
    }
}
