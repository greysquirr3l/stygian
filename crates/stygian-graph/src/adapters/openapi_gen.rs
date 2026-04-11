//! OpenAPI 3.0 spec generator from API discovery reports.
//!
//! Takes a [`DiscoveryReport`](crate::domain::discovery::DiscoveryReport) and
//! produces an [`openapiv3::OpenAPI`] specification.
//!
//! # Architecture
//!
//! This is an **adapter** that converts domain-level discovery types into
//! the `openapiv3` representation.  The domain layer has no knowledge of
//! OpenAPI; this adapter bridges that gap.
//!
//! # Example
//!
//! ```
//! use stygian_graph::adapters::openapi_gen::{OpenApiGenerator, SpecConfig};
//! use stygian_graph::domain::discovery::{DiscoveryReport, ResponseShape};
//! use serde_json::json;
//!
//! let mut report = DiscoveryReport::new();
//! report.add_endpoint("list_users", ResponseShape::from_body(&json!({"id": 1, "name": "A"})));
//!
//! let config = SpecConfig {
//!     title: "My API".into(),
//!     version: "1.0.0".into(),
//!     description: Some("Auto-discovered API".into()),
//!     servers: vec!["https://api.example.com".into()],
//! };
//!
//! let spec = OpenApiGenerator::generate(&report, &config);
//! assert_eq!(spec.info.title, "My API");
//! ```

use crate::domain::discovery::{DiscoveryReport, JsonType};
use indexmap::IndexMap;
use openapiv3::{
    Info, MediaType, OpenAPI, Operation, PathItem, ReferenceOr, Response, Schema, SchemaData,
    SchemaKind, Server, StatusCode, Type as OaType,
};
use std::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────────────────────
// SpecConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for the generated `OpenAPI 3.0` specification.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::openapi_gen::SpecConfig;
///
/// let config = SpecConfig {
///     title: "Pet Store".into(),
///     version: "2.0.0".into(),
///     description: Some("A sample API".into()),
///     servers: vec!["https://petstore.example.com/v2".into()],
/// };
/// ```
#[derive(Debug, Clone)]
pub struct SpecConfig {
    /// API title
    pub title: String,
    /// API version
    pub version: String,
    /// Optional description
    pub description: Option<String>,
    /// Server URLs
    pub servers: Vec<String>,
}

impl Default for SpecConfig {
    fn default() -> Self {
        Self {
            title: "Discovered API".into(),
            version: "0.1.0".into(),
            description: None,
            servers: Vec::new(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenApiGenerator
// ─────────────────────────────────────────────────────────────────────────────

/// Generates an `OpenAPI 3.0` specification from a [`DiscoveryReport`].
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::openapi_gen::{OpenApiGenerator, SpecConfig};
/// use stygian_graph::domain::discovery::{DiscoveryReport, ResponseShape};
/// use serde_json::json;
///
/// let mut report = DiscoveryReport::new();
/// report.add_endpoint("health", ResponseShape::from_body(&json!({"status": "ok"})));
///
/// let spec = OpenApiGenerator::generate(&report, &SpecConfig::default());
/// assert!(!spec.paths.paths.is_empty());
/// ```
pub struct OpenApiGenerator;

impl OpenApiGenerator {
    /// Generate an [`OpenAPI`] spec from a discovery report and config.
    ///
    /// Each endpoint in the report becomes a `GET` path.  Response schemas
    /// are inferred from the discovered field types.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::openapi_gen::{OpenApiGenerator, SpecConfig};
    /// use stygian_graph::domain::discovery::{DiscoveryReport, ResponseShape};
    /// use serde_json::json;
    ///
    /// let mut report = DiscoveryReport::new();
    /// report.add_endpoint("list_items", ResponseShape::from_body(&json!({"id": 1})));
    ///
    /// let spec = OpenApiGenerator::generate(&report, &SpecConfig::default());
    /// let yaml = serde_yaml::to_string(&spec).unwrap();
    /// assert!(yaml.contains("list_items"));
    /// ```
    #[must_use]
    pub fn generate(report: &DiscoveryReport, config: &SpecConfig) -> OpenAPI {
        let info = Info {
            title: config.title.clone(),
            version: config.version.clone(),
            description: config.description.clone(),
            ..Default::default()
        };

        let servers: Vec<Server> = config
            .servers
            .iter()
            .map(|url| Server {
                url: url.clone(),
                ..Default::default()
            })
            .collect();

        let mut paths = openapiv3::Paths::default();

        for (name, shape) in report.endpoints() {
            let path = format!("/{name}");
            let schema = Self::fields_to_schema(&shape.fields);

            let mut content = IndexMap::new();
            content.insert(
                "application/json".to_string(),
                MediaType {
                    schema: Some(ReferenceOr::Item(schema)),
                    ..Default::default()
                },
            );

            let response_200 = Response {
                description: format!("Successful response for {name}"),
                content,
                ..Default::default()
            };

            let mut responses = openapiv3::Responses::default();
            responses
                .responses
                .insert(StatusCode::Code(200), ReferenceOr::Item(response_200));

            let operation = Operation {
                operation_id: Some(name.clone()),
                responses,
                ..Default::default()
            };

            let path_item = PathItem {
                get: Some(operation),
                ..Default::default()
            };

            paths.paths.insert(path, ReferenceOr::Item(path_item));
        }

        OpenAPI {
            openapi: "3.0.3".to_string(),
            info,
            servers,
            paths,
            ..Default::default()
        }
    }

    /// Convert a field map to an `OpenAPI` object schema.
    fn fields_to_schema(fields: &BTreeMap<String, JsonType>) -> Schema {
        let properties: IndexMap<String, ReferenceOr<Box<Schema>>> = fields
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    ReferenceOr::Item(Box::new(Self::json_type_to_schema(v))),
                )
            })
            .collect();

        Schema {
            schema_data: SchemaData::default(),
            schema_kind: SchemaKind::Type(OaType::Object(openapiv3::ObjectType {
                properties,
                ..Default::default()
            })),
        }
    }

    /// Convert a [`JsonType`] to an `OpenAPI` schema.
    fn json_type_to_schema(jt: &JsonType) -> Schema {
        match jt {
            JsonType::Null => Schema {
                schema_data: SchemaData {
                    nullable: true,
                    ..Default::default()
                },
                schema_kind: SchemaKind::Type(OaType::String(openapiv3::StringType::default())),
            },
            JsonType::Bool => Schema {
                schema_data: SchemaData::default(),
                schema_kind: SchemaKind::Type(OaType::Boolean {}),
            },
            JsonType::Integer => Schema {
                schema_data: SchemaData::default(),
                schema_kind: SchemaKind::Type(OaType::Integer(openapiv3::IntegerType::default())),
            },
            JsonType::Float => Schema {
                schema_data: SchemaData::default(),
                schema_kind: SchemaKind::Type(OaType::Number(openapiv3::NumberType::default())),
            },
            JsonType::String | JsonType::Mixed => Schema {
                schema_data: SchemaData::default(),
                schema_kind: SchemaKind::Type(OaType::String(openapiv3::StringType::default())),
            },
            JsonType::Array(inner) => Schema {
                schema_data: SchemaData::default(),
                schema_kind: SchemaKind::Type(OaType::Array(openapiv3::ArrayType {
                    items: Some(ReferenceOr::Item(Box::new(Self::json_type_to_schema(
                        inner,
                    )))),
                    min_items: None,
                    max_items: None,
                    unique_items: false,
                })),
            },
            JsonType::Object(fields) => Self::fields_to_schema(fields),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::discovery::{DiscoveryReport, ResponseShape};
    use serde_json::json;

    #[test]
    fn generate_empty_report_produces_valid_spec() {
        let report = DiscoveryReport::new();
        let spec = OpenApiGenerator::generate(&report, &SpecConfig::default());
        assert_eq!(spec.openapi, "3.0.3");
        assert_eq!(spec.info.title, "Discovered API");
        assert!(spec.paths.paths.is_empty());
    }

    #[test]
    fn generate_single_endpoint() {
        let mut report = DiscoveryReport::new();
        report.add_endpoint(
            "list_items",
            ResponseShape::from_body(&json!({"id": 1, "name": "Widget"})),
        );

        let config = SpecConfig {
            title: "Test API".into(),
            version: "1.0.0".into(),
            description: Some("Test".into()),
            servers: vec!["https://api.test.com".into()],
        };

        let spec = OpenApiGenerator::generate(&report, &config);
        assert_eq!(spec.info.title, "Test API");
        assert_eq!(spec.servers.len(), 1);
        assert!(spec.paths.paths.contains_key("/list_items"));
    }

    #[test]
    fn generate_multiple_endpoints() {
        let mut report = DiscoveryReport::new();
        report.add_endpoint("users", ResponseShape::from_body(&json!({"id": 1})));
        report.add_endpoint("orders", ResponseShape::from_body(&json!({"total": 42.5})));

        let spec = OpenApiGenerator::generate(&report, &SpecConfig::default());
        assert_eq!(spec.paths.paths.len(), 2);
        assert!(spec.paths.paths.contains_key("/users"));
        assert!(spec.paths.paths.contains_key("/orders"));
    }

    #[test]
    fn spec_serialises_to_yaml() -> Result<(), Box<dyn std::error::Error>> {
        let mut report = DiscoveryReport::new();
        report.add_endpoint("health", ResponseShape::from_body(&json!({"status": "ok"})));

        let spec = OpenApiGenerator::generate(&report, &SpecConfig::default());
        let yaml = serde_yaml::to_string(&spec)?;
        assert!(yaml.contains("health"));
        assert!(yaml.contains("3.0.3"));
        Ok(())
    }
}
