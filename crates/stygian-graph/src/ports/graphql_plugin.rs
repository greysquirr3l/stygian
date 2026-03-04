//! `GraphQlTargetPlugin` port — one implementation per GraphQL API target.
//!
//! Each target (Jobber, GitHub, Shopify, …) registers a plugin that supplies
//! its endpoint, required version headers, default auth, and pagination defaults.
//! The generic [`crate::adapters::graphql::GraphQlService`] adapter resolves the
//! plugin at execution time; no target-specific knowledge lives in the adapter
//! itself.

use std::collections::HashMap;

use crate::ports::GraphQlAuth;

// ─────────────────────────────────────────────────────────────────────────────
// CostThrottleConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Static cost-throttle parameters for a GraphQL API target.
///
/// Set these to match the API documentation.  After the first successful
/// response the [`LiveBudget`](crate::adapters::graphql_throttle::LiveBudget)
/// will update itself from the `extensions.cost.throttleStatus` envelope.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::graphql_plugin::CostThrottleConfig;
///
/// let config = CostThrottleConfig {
///     max_points: 10_000.0,
///     restore_per_sec: 500.0,
///     min_available: 50.0,
///     max_delay_ms: 30_000,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CostThrottleConfig {
    /// Maximum point budget (e.g. `10_000.0` for Jobber / Shopify).
    pub max_points: f64,
    /// Points restored per second (e.g. `500.0`).
    pub restore_per_sec: f64,
    /// Minimum available points before a pre-flight delay is applied
    /// (default: `50.0`).
    pub min_available: f64,
    /// Upper bound on any computed pre-flight delay in milliseconds
    /// (default: `30_000`).
    pub max_delay_ms: u64,
}

impl Default for CostThrottleConfig {
    fn default() -> Self {
        Self {
            max_points: 10_000.0,
            restore_per_sec: 500.0,
            min_available: 50.0,
            max_delay_ms: 30_000,
        }
    }
}

/// A named GraphQL target that supplies connection defaults for a specific API.
///
/// Plugins are identified by their [`name`](Self::name) and loaded from the
/// [`GraphQlPluginRegistry`](crate::application::graphql_plugin_registry::GraphQlPluginRegistry)
/// at pipeline execution time.
///
/// # Example
///
/// ```rust
/// use std::collections::HashMap;
/// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
/// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
///
/// struct MyApiPlugin;
///
/// impl GraphQlTargetPlugin for MyApiPlugin {
///     fn name(&self) -> &str { "my-api" }
///     fn endpoint(&self) -> &str { "https://api.example.com/graphql" }
///     fn version_headers(&self) -> HashMap<String, String> {
///         [("X-API-VERSION".to_string(), "2025-01-01".to_string())].into()
///     }
///     fn default_auth(&self) -> Option<GraphQlAuth> { None }
/// }
/// ```
pub trait GraphQlTargetPlugin: Send + Sync {
    /// Canonical lowercase plugin name used in pipeline TOML: `plugin = "jobber"`.
    fn name(&self) -> &str;

    /// The GraphQL endpoint URL for this target.
    ///
    /// Used as the request URL when `ServiceInput.url` is empty.
    fn endpoint(&self) -> &str;

    /// Version or platform headers required by this API.
    ///
    /// Injected on every request. Plugin headers take precedence over
    /// ad-hoc `params.headers` for the same key.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::collections::HashMap;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
    ///
    /// struct JobberPlugin;
    /// impl GraphQlTargetPlugin for JobberPlugin {
    ///     fn name(&self) -> &str { "jobber" }
    ///     fn endpoint(&self) -> &str { "https://api.getjobber.com/api/graphql" }
    ///     fn version_headers(&self) -> HashMap<String, String> {
    ///         [("X-JOBBER-GRAPHQL-VERSION".to_string(), "2025-04-16".to_string())].into()
    ///     }
    /// }
    /// ```
    fn version_headers(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    /// Default auth to use when `params.auth` is absent.
    ///
    /// Implementations should read credentials from environment variables here.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::collections::HashMap;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
    ///
    /// struct SecurePlugin;
    /// impl GraphQlTargetPlugin for SecurePlugin {
    ///     fn name(&self) -> &str { "secure" }
    ///     fn endpoint(&self) -> &str { "https://api.secure.com/graphql" }
    ///     fn default_auth(&self) -> Option<GraphQlAuth> {
    ///         Some(GraphQlAuth {
    ///             kind: GraphQlAuthKind::Bearer,
    ///             token: "${env:SECURE_ACCESS_TOKEN}".to_string(),
    ///             header_name: None,
    ///         })
    ///     }
    /// }
    /// ```
    fn default_auth(&self) -> Option<GraphQlAuth> {
        None
    }

    /// Default page size for cursor-paginated queries.
    fn default_page_size(&self) -> usize {
        50
    }

    /// Whether this target uses Relay-style cursor pagination by default.
    fn supports_cursor_pagination(&self) -> bool {
        true
    }

    /// Human-readable description shown in `stygian plugins list`.
    #[allow(clippy::unnecessary_literal_bound)]
    fn description(&self) -> &str {
        ""
    }

    /// Optional cost-throttle configuration for proactive pre-flight delays.
    ///
    /// Return a populated [`CostThrottleConfig`] to enable the
    /// [`PluginBudget`](crate::adapters::graphql_throttle::PluginBudget)
    /// pre-flight delay mechanism in `GraphQlService`.
    ///
    /// The default implementation returns `None` (no proactive throttling).
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::collections::HashMap;
    /// use stygian_graph::ports::graphql_plugin::{GraphQlTargetPlugin, CostThrottleConfig};
    /// use stygian_graph::ports::GraphQlAuth;
    ///
    /// struct ThrottledPlugin;
    /// impl GraphQlTargetPlugin for ThrottledPlugin {
    ///     fn name(&self) -> &str { "throttled" }
    ///     fn endpoint(&self) -> &str { "https://api.example.com/graphql" }
    ///     fn cost_throttle_config(&self) -> Option<CostThrottleConfig> {
    ///         Some(CostThrottleConfig::default())
    ///     }
    /// }
    /// ```
    fn cost_throttle_config(&self) -> Option<CostThrottleConfig> {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unnecessary_literal_bound, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ports::GraphQlAuthKind;

    struct MinimalPlugin;

    impl GraphQlTargetPlugin for MinimalPlugin {
        fn name(&self) -> &str {
            "minimal"
        }
        fn endpoint(&self) -> &str {
            "https://api.example.com/graphql"
        }
    }

    #[test]
    fn default_methods_return_expected_values() {
        let plugin = MinimalPlugin;
        assert!(plugin.version_headers().is_empty());
        assert!(plugin.default_auth().is_none());
        assert_eq!(plugin.default_page_size(), 50);
        assert!(plugin.supports_cursor_pagination());
        assert_eq!(plugin.description(), "");
    }

    #[test]
    fn custom_version_headers_are_returned() {
        struct Versioned;
        impl GraphQlTargetPlugin for Versioned {
            fn name(&self) -> &str {
                "versioned"
            }
            fn endpoint(&self) -> &str {
                "https://api.v.com/graphql"
            }
            fn version_headers(&self) -> HashMap<String, String> {
                [("X-API-VERSION".to_string(), "v2".to_string())].into()
            }
        }
        let headers = Versioned.version_headers();
        assert_eq!(headers.get("X-API-VERSION").map(String::as_str), Some("v2"));
    }

    #[test]
    fn default_auth_can_be_overridden() {
        struct Authed;
        impl GraphQlTargetPlugin for Authed {
            fn name(&self) -> &str {
                "authed"
            }
            fn endpoint(&self) -> &str {
                "https://api.a.com/graphql"
            }
            fn default_auth(&self) -> Option<GraphQlAuth> {
                Some(GraphQlAuth {
                    kind: GraphQlAuthKind::Bearer,
                    token: "${env:TOKEN}".to_string(),
                    header_name: None,
                })
            }
        }
        let auth = Authed.default_auth().unwrap();
        assert_eq!(auth.kind, GraphQlAuthKind::Bearer);
        assert_eq!(auth.token, "${env:TOKEN}");
    }
}
