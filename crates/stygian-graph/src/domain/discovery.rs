//! API discovery domain types.
//!
//! Provides generic types for reverse-engineering undocumented REST APIs.
//! An API prober builds a [`DiscoveryReport`](crate::domain::discovery::DiscoveryReport) by analysing JSON responses
//! from target endpoints; the report can then be fed to
//! [`OpenApiGenerator`](crate::adapters::openapi_gen::OpenApiGenerator) to
//! produce an [`openapiv3::OpenAPI`] specification.
//!
//! These types are domain-pure — no I/O, no network calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// JsonType
// ─────────────────────────────────────────────────────────────────────────────

/// Recursive enum representing an inferred JSON Schema type from a
/// [`serde_json::Value`].
///
/// # Example
///
/// ```
/// use stygian_graph::domain::discovery::JsonType;
/// use serde_json::json;
///
/// let t = JsonType::infer(&json!(42));
/// assert_eq!(t, JsonType::Integer);
///
/// let t = JsonType::infer(&json!({"name": "Alice", "age": 30}));
/// assert!(matches!(t, JsonType::Object(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JsonType {
    /// JSON `null`
    Null,
    /// JSON boolean
    Bool,
    /// Integer (no fractional part)
    Integer,
    /// Floating-point number
    Float,
    /// JSON string
    String,
    /// Homogeneous array with inferred item type
    Array(Box<JsonType>),
    /// Object with field name → inferred type mapping
    Object(BTreeMap<std::string::String, JsonType>),
    /// Mixed / conflicting types (e.g. field is sometimes string, sometimes int)
    Mixed,
}

impl JsonType {
    /// Infer the [`JsonType`] of a [`serde_json::Value`].
    ///
    /// For arrays, the item type is inferred from all elements; conflicting
    /// element types collapse to [`JsonType::Mixed`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::JsonType;
    /// use serde_json::json;
    ///
    /// assert_eq!(JsonType::infer(&json!("hello")), JsonType::String);
    /// assert_eq!(JsonType::infer(&json!(true)), JsonType::Bool);
    /// assert_eq!(JsonType::infer(&json!(null)), JsonType::Null);
    /// assert_eq!(JsonType::infer(&json!(3.14)), JsonType::Float);
    /// ```
    #[must_use]
    pub fn infer(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Bool,
            Value::Number(n) => {
                if n.is_f64() && n.as_i64().is_none() && n.as_u64().is_none() {
                    Self::Float
                } else {
                    Self::Integer
                }
            }
            Value::String(_) => Self::String,
            Value::Array(arr) => {
                if arr.is_empty() {
                    return Self::Array(Box::new(Self::Mixed));
                }
                let first = Self::infer(&arr[0]);
                let uniform = arr.iter().skip(1).all(|v| Self::infer(v) == first);
                if uniform {
                    Self::Array(Box::new(first))
                } else {
                    Self::Array(Box::new(Self::Mixed))
                }
            }
            Value::Object(map) => {
                let fields = map
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::infer(v)))
                    .collect();
                Self::Object(fields)
            }
        }
    }

    /// Return the JSON Schema type string for this variant.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::JsonType;
    ///
    /// assert_eq!(JsonType::String.schema_type(), "string");
    /// assert_eq!(JsonType::Integer.schema_type(), "integer");
    /// ```
    #[must_use]
    pub const fn schema_type(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool => "boolean",
            Self::Integer => "integer",
            Self::Float => "number",
            Self::String => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
            Self::Mixed => "string", // fallback
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PaginationStyle
// ─────────────────────────────────────────────────────────────────────────────

/// Detected pagination envelope style from API response inspection.
///
/// # Example
///
/// ```
/// use stygian_graph::domain::discovery::PaginationStyle;
///
/// let style = PaginationStyle {
///     has_data_wrapper: true,
///     has_current_page: true,
///     has_total_pages: true,
///     has_last_page: false,
///     has_total: true,
///     has_per_page: true,
/// };
/// assert!(style.is_paginated());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationStyle {
    /// Response wraps data in a `data` key
    pub has_data_wrapper: bool,
    /// Contains a `current_page` or `page` field
    pub has_current_page: bool,
    /// Contains a `total_pages` field
    pub has_total_pages: bool,
    /// Contains a `last_page` field
    pub has_last_page: bool,
    /// Contains a `total` or `total_count` field
    pub has_total: bool,
    /// Contains a `per_page` or `page_size` field
    pub has_per_page: bool,
}

impl PaginationStyle {
    /// Returns `true` if any pagination signal was detected.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::PaginationStyle;
    ///
    /// let empty = PaginationStyle::default();
    /// assert!(!empty.is_paginated());
    /// ```
    #[must_use]
    pub const fn is_paginated(&self) -> bool {
        self.has_current_page
            || self.has_total_pages
            || self.has_last_page
            || self.has_total
            || self.has_per_page
    }

    /// Detect pagination style from a JSON response body.
    ///
    /// Looks for common pagination envelope keys at the top level.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::PaginationStyle;
    /// use serde_json::json;
    ///
    /// let body = json!({"data": [], "current_page": 1, "total": 42, "per_page": 25});
    /// let style = PaginationStyle::detect(&body);
    /// assert!(style.has_data_wrapper);
    /// assert!(style.has_current_page);
    /// assert!(style.has_total);
    /// ```
    #[must_use]
    pub fn detect(body: &Value) -> Self {
        let obj = match body.as_object() {
            Some(o) => o,
            None => return Self::default(),
        };
        Self {
            has_data_wrapper: obj.contains_key("data"),
            has_current_page: obj.contains_key("current_page") || obj.contains_key("page"),
            has_total_pages: obj.contains_key("total_pages"),
            has_last_page: obj.contains_key("last_page"),
            has_total: obj.contains_key("total") || obj.contains_key("total_count"),
            has_per_page: obj.contains_key("per_page") || obj.contains_key("page_size"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ResponseShape
// ─────────────────────────────────────────────────────────────────────────────

/// Shape of a single discovered endpoint's response.
///
/// # Example
///
/// ```
/// use stygian_graph::domain::discovery::{ResponseShape, PaginationStyle, JsonType};
/// use serde_json::json;
/// use std::collections::BTreeMap;
///
/// let shape = ResponseShape {
///     fields: BTreeMap::from([("id".into(), JsonType::Integer), ("name".into(), JsonType::String)]),
///     sample: Some(json!({"id": 1, "name": "Widget"})),
///     pagination_detected: true,
///     pagination_style: PaginationStyle::default(),
/// };
/// assert_eq!(shape.fields.len(), 2);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseShape {
    /// Inferred field types
    pub fields: BTreeMap<String, JsonType>,
    /// Optional representative sample value
    pub sample: Option<Value>,
    /// Whether pagination was detected
    pub pagination_detected: bool,
    /// Pagination envelope style details
    pub pagination_style: PaginationStyle,
}

impl ResponseShape {
    /// Build a `ResponseShape` by analysing a JSON response body.
    ///
    /// If the body is an object with a `data` key that is an array,
    /// fields are inferred from the first array element.  Otherwise
    /// the top-level object fields are used.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::ResponseShape;
    /// use serde_json::json;
    ///
    /// let body = json!({"data": [{"id": 1, "name": "A"}], "total": 50, "per_page": 25});
    /// let shape = ResponseShape::from_body(&body);
    /// assert!(shape.pagination_detected);
    /// assert!(shape.fields.contains_key("id"));
    /// ```
    #[must_use]
    pub fn from_body(body: &Value) -> Self {
        let pagination_style = PaginationStyle::detect(body);
        let pagination_detected = pagination_style.is_paginated();

        // Try to extract fields from data[0] if it's a wrapped array
        let (fields, sample) = if let Some(arr) = body.get("data").and_then(Value::as_array) {
            if let Some(first) = arr.first() {
                let inferred = match JsonType::infer(first) {
                    JsonType::Object(m) => m,
                    other => BTreeMap::from([("value".into(), other)]),
                };
                (inferred, Some(first.clone()))
            } else {
                (BTreeMap::new(), None)
            }
        } else {
            match JsonType::infer(body) {
                JsonType::Object(m) => {
                    let sample = Some(body.clone());
                    (m, sample)
                }
                other => (
                    BTreeMap::from([("value".into(), other)]),
                    Some(body.clone()),
                ),
            }
        };

        Self {
            fields,
            sample,
            pagination_detected,
            pagination_style,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DiscoveryReport
// ─────────────────────────────────────────────────────────────────────────────

/// Collection of [`ResponseShape`]s keyed by endpoint name.
///
/// A discovery probe fills this report and passes it to
/// [`OpenApiGenerator`](crate::adapters::openapi_gen::OpenApiGenerator).
///
/// # Example
///
/// ```
/// use stygian_graph::domain::discovery::{DiscoveryReport, ResponseShape};
/// use serde_json::json;
///
/// let mut report = DiscoveryReport::new();
/// let body = json!({"id": 1, "name": "Test"});
/// report.add_endpoint("get_items", ResponseShape::from_body(&body));
/// assert_eq!(report.endpoints().len(), 1);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryReport {
    endpoints: BTreeMap<String, ResponseShape>,
}

impl DiscoveryReport {
    /// Create an empty report.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::DiscoveryReport;
    ///
    /// let report = DiscoveryReport::new();
    /// assert!(report.endpoints().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a discovered endpoint shape.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::{DiscoveryReport, ResponseShape};
    /// use serde_json::json;
    ///
    /// let mut report = DiscoveryReport::new();
    /// report.add_endpoint("users", ResponseShape::from_body(&json!({"id": 1})));
    /// ```
    pub fn add_endpoint(&mut self, name: &str, shape: ResponseShape) {
        self.endpoints.insert(name.to_string(), shape);
    }

    /// Return a view of all discovered endpoints.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::domain::discovery::DiscoveryReport;
    ///
    /// let report = DiscoveryReport::new();
    /// assert!(report.endpoints().is_empty());
    /// ```
    #[must_use]
    pub fn endpoints(&self) -> &BTreeMap<String, ResponseShape> {
        &self.endpoints
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_type_infer_primitives() {
        assert_eq!(JsonType::infer(&json!(null)), JsonType::Null);
        assert_eq!(JsonType::infer(&json!(true)), JsonType::Bool);
        assert_eq!(JsonType::infer(&json!(42)), JsonType::Integer);
        assert_eq!(JsonType::infer(&json!(3.14)), JsonType::Float);
        assert_eq!(JsonType::infer(&json!("hello")), JsonType::String);
    }

    #[test]
    fn json_type_infer_array_uniform() {
        let t = JsonType::infer(&json!([1, 2, 3]));
        assert_eq!(t, JsonType::Array(Box::new(JsonType::Integer)));
    }

    #[test]
    fn json_type_infer_array_mixed() {
        let t = JsonType::infer(&json!([1, "two", 3]));
        assert_eq!(t, JsonType::Array(Box::new(JsonType::Mixed)));
    }

    #[test]
    fn json_type_infer_object() {
        let t = JsonType::infer(&json!({"name": "Alice", "age": 30}));
        match t {
            JsonType::Object(fields) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields["name"], JsonType::String);
                assert_eq!(fields["age"], JsonType::Integer);
            }
            other => panic!("expected Object, got {other:?}"),
        }
    }

    #[test]
    fn pagination_style_detect_common_envelope() {
        let body = json!({
            "data": [{"id": 1}],
            "current_page": 1,
            "total": 100,
            "per_page": 25,
        });
        let style = PaginationStyle::detect(&body);
        assert!(style.has_data_wrapper);
        assert!(style.has_current_page);
        assert!(style.has_total);
        assert!(style.has_per_page);
        assert!(style.is_paginated());
    }

    #[test]
    fn pagination_style_detect_none() {
        let body = json!({"items": [{"id": 1}]});
        let style = PaginationStyle::detect(&body);
        assert!(!style.is_paginated());
    }

    #[test]
    fn response_shape_from_wrapped_body() {
        let body = json!({
            "data": [{"id": 1, "name": "Test"}],
            "total": 42,
            "per_page": 25,
        });
        let shape = ResponseShape::from_body(&body);
        assert!(shape.pagination_detected);
        assert!(shape.fields.contains_key("id"));
        assert!(shape.fields.contains_key("name"));
    }

    #[test]
    fn response_shape_from_flat_body() {
        let body = json!({"id": 1, "name": "Test"});
        let shape = ResponseShape::from_body(&body);
        assert!(!shape.pagination_detected);
        assert!(shape.fields.contains_key("id"));
    }

    #[test]
    fn discovery_report_roundtrip() {
        let mut report = DiscoveryReport::new();
        let body = json!({"data": [{"id": 1}], "total": 1, "per_page": 25});
        report.add_endpoint("items", ResponseShape::from_body(&body));

        assert_eq!(report.endpoints().len(), 1);
        assert!(report.endpoints().contains_key("items"));
    }
}
