//! Extraction template, request, and result types

use crate::domain::idempotency::IdempotencyKey;
use crate::domain::selector::Selector;
use crate::domain::transformation::Transformation;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// A named region within a template to extract data from
///
/// Each region represents a distinct zone on the page with its own
/// selectors and transformations.
///
/// # Example
///
/// ```
/// use stygian_plugin::domain::Region;
/// use stygian_plugin::domain::Selector;
///
/// let region = Region {
///     name: "product-title".to_string(),
///     selector: Selector::css(".product-name".to_string()),
///     schema: serde_json::json!({"type": "string"}),
///     transformations: vec![],
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    /// Region name (e.g., "product-title", "price", "rating")
    pub name: String,

    /// Primary selector (`CSS` or `XPath`) to locate the element
    pub selector: Selector,

    /// JSON schema describing the expected output shape
    pub schema: Value,

    /// Ordered transformations to apply to extracted values
    pub transformations: Vec<Transformation>,
}

impl Region {
    /// Create a new region with minimal configuration
    pub fn new(name: impl Into<String>, selector: Selector, schema: Value) -> Self {
        Self {
            name: name.into(),
            selector,
            schema,
            transformations: vec![],
        }
    }

    /// Add a transformation to the pipeline
    #[must_use]
    pub fn with_transformation(mut self, transformation: Transformation) -> Self {
        self.transformations.push(transformation);
        self
    }

    /// Validate region configuration
    pub fn validate(&self) -> crate::Result<()> {
        if self.name.is_empty() {
            return Err(crate::error::PluginError::TemplateValidationError(
                "region name cannot be empty".to_string(),
            ));
        }
        if !self.schema.is_object() {
            return Err(crate::error::PluginError::TemplateValidationError(format!(
                "region schema must be a JSON object, got {}",
                self.schema.get("type").unwrap_or(&Value::Null)
            )));
        }
        // Validate the selector syntax
        self.selector.validate()?;
        Ok(())
    }
}

/// A reusable extraction template defining how to extract data from a page
///
/// Templates combine multiple regions, each with selectors and transformations.
/// A template is the core unit of plugin configuration and is persisted for reuse.
///
/// # Example
///
/// ```
/// use stygian_plugin::domain::{ExtractionTemplate, Region, Selector};
/// use serde_json::json;
///
/// let template = ExtractionTemplate {
///     id: uuid::Uuid::new_v4(),
///     name: "Product Listing".to_string(),
///     description: Some("Extract product cards from a listing page".to_string()),
///     regions: vec![],
///     metadata: Default::default(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionTemplate {
    /// Unique identifier for this template
    pub id: uuid::Uuid,

    /// User-friendly template name
    pub name: String,

    /// Optional description
    pub description: Option<String>,

    /// Regions (named extraction zones) in this template
    pub regions: Vec<Region>,

    /// Metadata (timestamps, version, etc.)
    pub metadata: TemplateMetadata,
}

/// Metadata about a template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMetadata {
    /// When template was created
    pub created_at: DateTime<Utc>,

    /// When template was last modified
    pub updated_at: DateTime<Utc>,

    /// When template was last used
    pub last_used_at: Option<DateTime<Utc>>,

    /// Number of times this template has been used
    pub usage_count: u64,

    /// Template version (for migration purposes)
    pub version: u32,

    /// Optional user-defined tags
    pub tags: Vec<String>,
}

impl Default for TemplateMetadata {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            created_at: now,
            updated_at: now,
            last_used_at: None,
            usage_count: 0,
            version: 1,
            tags: vec![],
        }
    }
}

impl ExtractionTemplate {
    /// Create a new template with defaults
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            description: None,
            regions: vec![],
            metadata: TemplateMetadata::default(),
        }
    }

    /// Add a region to this template
    #[must_use]
    pub fn with_region(mut self, region: Region) -> Self {
        self.regions.push(region);
        self
    }

    /// Set template description
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set template tags
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.metadata.tags = tags;
        self
    }

    /// Validate the entire template
    pub fn validate(&self) -> crate::Result<()> {
        if self.name.is_empty() {
            return Err(crate::error::PluginError::TemplateValidationError(
                "template name cannot be empty".to_string(),
            ));
        }
        if self.regions.is_empty() {
            return Err(crate::error::PluginError::TemplateValidationError(
                "template must have at least one region".to_string(),
            ));
        }
        for region in &self.regions {
            region.validate()?;
        }
        Ok(())
    }

    /// Update usage statistics
    pub fn mark_used(&mut self) {
        self.metadata.usage_count += 1;
        self.metadata.last_used_at = Some(Utc::now());
        self.metadata.updated_at = Utc::now();
    }
}

/// Request to extract data from a page using a template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionRequest {
    /// Template to use for extraction
    pub template: ExtractionTemplate,

    /// Target URL (for context/logging)
    pub url: String,

    /// HTML content of the page to extract from
    pub html: String,

    /// Idempotency key for safe retries
    pub idempotency_key: IdempotencyKey,

    /// Timeout in milliseconds
    pub timeout_ms: u64,

    /// Optional extraction context (arbitrary JSON)
    pub context: Option<Value>,
}

impl ExtractionRequest {
    /// Create a new extraction request
    pub fn new(
        template: ExtractionTemplate,
        url: impl Into<String>,
        html: impl Into<String>,
    ) -> Self {
        Self {
            template,
            url: url.into(),
            html: html.into(),
            idempotency_key: IdempotencyKey::new(),
            timeout_ms: 30_000,
            context: None,
        }
    }

    /// Set idempotency key
    #[must_use]
    pub const fn with_idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = key;
        self
    }

    /// Set timeout
    #[must_use]
    pub const fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Set context
    #[must_use]
    pub fn with_context(mut self, context: Value) -> Self {
        self.context = Some(context);
        self
    }

    /// Validate the request
    pub fn validate(&self) -> crate::Result<()> {
        self.template.validate()?;
        if self.url.is_empty() {
            return Err(crate::error::PluginError::ExtractionError(
                "URL cannot be empty".to_string(),
            ));
        }
        if self.html.is_empty() {
            return Err(crate::error::PluginError::ExtractionError(
                "HTML cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Result of a successful extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    /// Extracted data keyed by region name
    pub data: HashMap<String, Value>,

    /// Metadata about the extraction
    pub metadata: ExtractionMetadata,
}

/// Metadata about an extraction result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionMetadata {
    /// Idempotency key used
    pub idempotency_key: IdempotencyKey,

    /// When extraction was completed
    pub completed_at: DateTime<Utc>,

    /// Elapsed time in milliseconds
    pub elapsed_ms: u64,

    /// Success rate for selectors (0-100)
    pub selector_success_rate: f32,

    /// Per-region extraction status
    pub region_status: HashMap<String, RegionStatus>,

    /// Optional error details
    pub errors: Vec<String>,
}

/// Status of extraction for a single region
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionStatus {
    /// Whether extraction succeeded
    pub success: bool,

    /// Number of elements matched
    pub matched_count: usize,

    /// Error message if failed
    pub error: Option<String>,
}

impl ExtractionResult {
    /// Create a new extraction result
    pub fn new(idempotency_key: IdempotencyKey) -> Self {
        Self {
            data: HashMap::new(),
            metadata: ExtractionMetadata {
                idempotency_key,
                completed_at: Utc::now(),
                elapsed_ms: 0,
                selector_success_rate: 0.0,
                region_status: HashMap::new(),
                errors: vec![],
            },
        }
    }

    /// Add extracted data for a region
    #[must_use]
    pub fn with_region_data(mut self, region_name: impl Into<String>, data: Value) -> Self {
        self.data.insert(region_name.into(), data);
        self
    }

    /// Add an error
    #[must_use]
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.metadata.errors.push(error.into());
        self
    }

    /// Update elapsed time
    #[must_use]
    pub const fn set_elapsed_ms(mut self, ms: u64) -> Self {
        self.metadata.elapsed_ms = ms;
        self
    }

    /// Calculate and set selector success rate
    #[expect(
        clippy::cast_precision_loss,
        reason = "region counts are small enough to be safe as f32"
    )]
    pub fn calculate_success_rate(&mut self) {
        if self.metadata.region_status.is_empty() {
            self.metadata.selector_success_rate = 100.0;
            return;
        }
        let successful = self
            .metadata
            .region_status
            .values()
            .filter(|status| status.success)
            .count();
        self.metadata.selector_success_rate =
            (successful as f32 / self.metadata.region_status.len() as f32) * 100.0;
    }

    /// Check if extraction was fully successful
    pub fn is_fully_successful(&self) -> bool {
        self.metadata.selector_success_rate >= 100.0 && self.metadata.errors.is_empty()
    }
}
