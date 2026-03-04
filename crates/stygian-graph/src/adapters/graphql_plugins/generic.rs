//! Generic GraphQL target plugin with a fluent builder API.
//!
//! Use `GenericGraphQlPlugin` when you need a quick, ad-hoc plugin without
//! writing a dedicated implementation struct.  Supply the endpoint, optional
//! auth, headers, and cost-throttle configuration via the builder.
//!
//! # Example
//!
//! ```rust
//! use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
//! use stygian_graph::adapters::graphql_throttle::CostThrottleConfig;
//! use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
//! use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
//!
//! let plugin = GenericGraphQlPlugin::builder()
//!     .name("github")
//!     .endpoint("https://api.github.com/graphql")
//!     .auth(GraphQlAuth {
//!         kind: GraphQlAuthKind::Bearer,
//!         token: "${env:GITHUB_TOKEN}".to_string(),
//!         header_name: None,
//!     })
//!     .header("X-Github-Next-Global-ID", "1")
//!     .cost_throttle(CostThrottleConfig::default())
//!     .page_size(30)
//!     .description("GitHub GraphQL API")
//!     .build()
//!     .expect("required fields: name and endpoint");
//!
//! assert_eq!(plugin.name(), "github");
//! assert_eq!(plugin.default_page_size(), 30);
//! ```

use std::collections::HashMap;

use crate::ports::graphql_plugin::{CostThrottleConfig, GraphQlTargetPlugin};
use crate::ports::{GraphQlAuth, GraphQlAuthKind};

// ─────────────────────────────────────────────────────────────────────────────
// Plugin struct
// ─────────────────────────────────────────────────────────────────────────────

/// A fully generic GraphQL target plugin built via [`GenericGraphQlPluginBuilder`].
///
/// Implements [`GraphQlTargetPlugin`] and can be registered with
/// `GraphQlPluginRegistry` like any other plugin.
#[derive(Debug, Clone)]
pub struct GenericGraphQlPlugin {
    name: String,
    endpoint: String,
    headers: HashMap<String, String>,
    auth: Option<GraphQlAuth>,
    throttle: Option<CostThrottleConfig>,
    page_size: usize,
    description: String,
}

impl GenericGraphQlPlugin {
    /// Return a fresh [`GenericGraphQlPluginBuilder`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    ///
    /// let plugin = GenericGraphQlPlugin::builder()
    ///     .name("my-api")
    ///     .endpoint("https://api.example.com/graphql")
    ///     .build()
    ///     .expect("name and endpoint are required");
    ///
    /// assert_eq!(plugin.name(), "my-api");
    /// ```
    #[must_use]
    pub fn builder() -> GenericGraphQlPluginBuilder {
        GenericGraphQlPluginBuilder::default()
    }

    /// Return the configured cost-throttle config if any.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    /// use stygian_graph::adapters::graphql_throttle::CostThrottleConfig;
    ///
    /// let plugin = GenericGraphQlPlugin::builder()
    ///     .name("api")
    ///     .endpoint("https://api.example.com/graphql")
    ///     .cost_throttle(CostThrottleConfig::default())
    ///     .build()
    ///     .expect("ok");
    ///
    /// assert!(plugin.cost_throttle_config().is_some());
    /// ```
    #[must_use]
    pub const fn cost_throttle_config(&self) -> Option<&CostThrottleConfig> {
        self.throttle.as_ref()
    }
}

impl GraphQlTargetPlugin for GenericGraphQlPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn version_headers(&self) -> HashMap<String, String> {
        self.headers.clone()
    }

    fn default_auth(&self) -> Option<GraphQlAuth> {
        self.auth.clone()
    }

    fn default_page_size(&self) -> usize {
        self.page_size
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn supports_cursor_pagination(&self) -> bool {
        true
    }

    fn cost_throttle_config(&self) -> Option<CostThrottleConfig> {
        self.throttle.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Builder
// ─────────────────────────────────────────────────────────────────────────────

/// Builder for [`GenericGraphQlPlugin`].
///
/// Obtain via [`GenericGraphQlPlugin::builder()`].  The only required fields
/// are `name` and `endpoint`; everything else has sensible defaults.
#[derive(Debug, Default)]
pub struct GenericGraphQlPluginBuilder {
    name: Option<String>,
    endpoint: Option<String>,
    headers: HashMap<String, String>,
    auth: Option<GraphQlAuth>,
    throttle: Option<CostThrottleConfig>,
    page_size: usize,
    description: String,
}

impl GenericGraphQlPluginBuilder {
    /// Set the plugin name (required).
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let _builder = GenericGraphQlPlugin::builder().name("my-api");
    /// ```
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the GraphQL endpoint URL (required).
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let _builder = GenericGraphQlPlugin::builder()
    ///     .endpoint("https://api.example.com/graphql");
    /// ```
    #[must_use]
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Add a single request header.
    ///
    /// May be called multiple times to accumulate headers.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let _builder = GenericGraphQlPlugin::builder()
    ///     .header("X-Api-Version", "2025-01-01")
    ///     .header("Accept-Language", "en");
    /// ```
    #[must_use]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Replace all headers with a pre-built map.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::collections::HashMap;
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let headers: HashMap<_, _> = [("X-Version", "1")].into_iter()
    ///     .map(|(k, v)| (k.to_string(), v.to_string()))
    ///     .collect();
    /// let _builder = GenericGraphQlPlugin::builder().headers(headers);
    /// ```
    #[must_use]
    pub fn headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// Set the default auth credentials.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    /// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
    ///
    /// let _builder = GenericGraphQlPlugin::builder()
    ///     .auth(GraphQlAuth {
    ///         kind: GraphQlAuthKind::Bearer,
    ///         token: "${env:GITHUB_TOKEN}".to_string(),
    ///         header_name: None,
    ///     });
    /// ```
    #[must_use]
    pub fn auth(mut self, auth: GraphQlAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Convenience helper: set a Bearer-token auth from a string.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let _builder = GenericGraphQlPlugin::builder()
    ///     .bearer_auth("${env:MY_TOKEN}");
    /// ```
    #[must_use]
    pub fn bearer_auth(mut self, token: impl Into<String>) -> Self {
        self.auth = Some(GraphQlAuth {
            kind: GraphQlAuthKind::Bearer,
            token: token.into(),
            header_name: None,
        });
        self
    }

    /// Attach a cost-throttle configuration for proactive pre-flight delays.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    /// use stygian_graph::adapters::graphql_throttle::CostThrottleConfig;
    ///
    /// let _builder = GenericGraphQlPlugin::builder()
    ///     .cost_throttle(CostThrottleConfig::default());
    /// ```
    #[must_use]
    pub const fn cost_throttle(mut self, throttle: CostThrottleConfig) -> Self {
        self.throttle = Some(throttle);
        self
    }

    /// Override the default page size (default: `50`).
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let _builder = GenericGraphQlPlugin::builder().page_size(30);
    /// ```
    #[must_use]
    pub const fn page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size;
        self
    }

    /// Set a human-readable description of the plugin.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    ///
    /// let _builder = GenericGraphQlPlugin::builder()
    ///     .description("GitHub public API v4");
    /// ```
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Consume the builder and produce a [`GenericGraphQlPlugin`].
    ///
    /// Returns `Err` if `name` or `endpoint` were not set.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    ///
    /// let plugin = GenericGraphQlPlugin::builder()
    ///     .name("github")
    ///     .endpoint("https://api.github.com/graphql")
    ///     .build()
    ///     .expect("ok");
    ///
    /// assert_eq!(plugin.name(), "github");
    /// ```
    pub fn build(self) -> Result<GenericGraphQlPlugin, BuildError> {
        Ok(GenericGraphQlPlugin {
            name: self.name.ok_or(BuildError::MissingName)?,
            endpoint: self.endpoint.ok_or(BuildError::MissingEndpoint)?,
            headers: self.headers,
            auth: self.auth,
            throttle: self.throttle,
            page_size: if self.page_size == 0 {
                50
            } else {
                self.page_size
            },
            description: self.description,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BuildError
// ─────────────────────────────────────────────────────────────────────────────

/// Errors that can occur when building a [`GenericGraphQlPlugin`].
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The `name` field was not set.
    #[error("plugin name is required — call .name(\"...\")")]
    MissingName,
    /// The `endpoint` field was not set.
    #[error("plugin endpoint is required — call .endpoint(\"...\")")]
    MissingEndpoint,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn minimal_plugin() -> GenericGraphQlPlugin {
        GenericGraphQlPlugin::builder()
            .name("test")
            .endpoint("https://api.example.com/graphql")
            .build()
            .unwrap()
    }

    #[test]
    fn builder_minimal_roundtrip() {
        let p = minimal_plugin();
        assert_eq!(p.name(), "test");
        assert_eq!(p.endpoint(), "https://api.example.com/graphql");
        assert_eq!(p.default_page_size(), 50); // default
        assert!(p.default_auth().is_none());
        assert!(p.cost_throttle_config().is_none());
        assert!(p.version_headers().is_empty());
    }

    #[test]
    fn builder_full_roundtrip() {
        let plugin = GenericGraphQlPlugin::builder()
            .name("github")
            .endpoint("https://api.github.com/graphql")
            .bearer_auth("ghp_test")
            .header("X-Github-Next-Global-ID", "1")
            .cost_throttle(CostThrottleConfig::default())
            .page_size(30)
            .description("GitHub v4")
            .build()
            .unwrap();

        assert_eq!(plugin.name(), "github");
        assert_eq!(plugin.default_page_size(), 30);
        assert_eq!(plugin.description(), "GitHub v4");
        assert!(plugin.default_auth().is_some());
        assert!(plugin.cost_throttle_config().is_some());
        let headers = plugin.version_headers();
        assert_eq!(
            headers.get("X-Github-Next-Global-ID").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn builder_error_missing_name() {
        let result = GenericGraphQlPlugin::builder()
            .endpoint("https://api.example.com/graphql")
            .build();
        assert!(matches!(result, Err(BuildError::MissingName)));
    }

    #[test]
    fn builder_error_missing_endpoint() {
        let result = GenericGraphQlPlugin::builder().name("api").build();
        assert!(matches!(result, Err(BuildError::MissingEndpoint)));
    }

    #[test]
    fn page_size_zero_defaults_to_50() {
        let plugin = GenericGraphQlPlugin::builder()
            .name("api")
            .endpoint("https://api.example.com/graphql")
            .page_size(0)
            .build()
            .unwrap();
        assert_eq!(plugin.default_page_size(), 50);
    }

    #[test]
    fn headers_map_replacement() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert("X-Foo".to_string(), "bar".to_string());
        let plugin = GenericGraphQlPlugin::builder()
            .name("api")
            .endpoint("https://api.example.com/graphql")
            .headers(map)
            .build()
            .unwrap();
        assert_eq!(
            plugin.version_headers().get("X-Foo").map(String::as_str),
            Some("bar")
        );
    }
}
