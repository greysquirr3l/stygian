//! `GraphQlTargetPlugin` port — one implementation per GraphQL API target.
//!
//! Each target (Jobber, GitHub, Shopify, …) registers a plugin that supplies
//! its endpoint, required version headers, default auth, and pagination defaults.
//! The generic [`crate::adapters::graphql::GraphQlService`] adapter resolves the
//! plugin at execution time; no target-specific knowledge lives in the adapter
//! itself.

use std::collections::HashMap;

use crate::ports::GraphQlAuth;

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
/// use mycelium_graph::ports::graphql_plugin::GraphQlTargetPlugin;
/// use mycelium_graph::ports::{GraphQlAuth, GraphQlAuthKind};
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
    /// use mycelium_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use mycelium_graph::ports::{GraphQlAuth, GraphQlAuthKind};
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
    /// use mycelium_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use mycelium_graph::ports::{GraphQlAuth, GraphQlAuthKind};
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

    /// Human-readable description shown in `mycelium plugins list`.
    fn description(&self) -> &str {
        ""
    }
}

#[cfg(test)]
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
