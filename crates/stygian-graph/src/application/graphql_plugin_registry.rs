//! Registry for named GraphQL target plugins.
//!
//! Plugins are registered at startup and looked up by name when the pipeline
//! executor resolves a `kind = "graphql"` service that carries a
//! `plugin = "<name>"` field.

use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::error::{ConfigError, StygianError, Result};
use crate::ports::graphql_plugin::GraphQlTargetPlugin;

/// A registry of named [`GraphQlTargetPlugin`] implementations.
///
/// # Example
///
/// ```rust
/// use stygian_graph::application::graphql_plugin_registry::GraphQlPluginRegistry;
/// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
/// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
/// use std::collections::HashMap;
/// use std::sync::Arc;
///
/// struct DemoPlugin;
/// impl GraphQlTargetPlugin for DemoPlugin {
///     fn name(&self) -> &str { "demo" }
///     fn endpoint(&self) -> &str { "https://demo.example.com/graphql" }
/// }
///
/// let mut registry = GraphQlPluginRegistry::new();
/// registry.register(Arc::new(DemoPlugin));
/// let plugin = registry.get("demo").unwrap();
/// assert_eq!(plugin.endpoint(), "https://demo.example.com/graphql");
/// ```
pub struct GraphQlPluginRegistry {
    plugins: HashMap<String, Arc<dyn GraphQlTargetPlugin>>,
}

impl GraphQlPluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Register a plugin. Replaces any existing registration with the same name.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::application::graphql_plugin_registry::GraphQlPluginRegistry;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
    /// use std::collections::HashMap;
    /// use std::sync::Arc;
    ///
    /// struct P;
    /// impl GraphQlTargetPlugin for P {
    ///     fn name(&self) -> &str { "p" }
    ///     fn endpoint(&self) -> &str { "https://p.example.com/graphql" }
    /// }
    ///
    /// let mut registry = GraphQlPluginRegistry::new();
    /// registry.register(Arc::new(P));
    /// ```
    pub fn register(&mut self, plugin: Arc<dyn GraphQlTargetPlugin>) {
        self.plugins.insert(plugin.name().to_owned(), plugin);
    }

    /// Look up a plugin by name.
    ///
    /// # Errors
    ///
    /// Returns [`StygianError::Config`] wrapping [`ConfigError::MissingConfig`]
    /// if no plugin with that name has been registered.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::application::graphql_plugin_registry::GraphQlPluginRegistry;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
    /// use std::collections::HashMap;
    /// use std::sync::Arc;
    ///
    /// struct P;
    /// impl GraphQlTargetPlugin for P {
    ///     fn name(&self) -> &str { "p" }
    ///     fn endpoint(&self) -> &str { "https://p.example.com/graphql" }
    /// }
    ///
    /// let mut registry = GraphQlPluginRegistry::new();
    /// registry.register(Arc::new(P));
    /// assert!(registry.get("p").is_ok());
    /// assert!(registry.get("missing").is_err());
    /// ```
    pub fn get(&self, name: &str) -> Result<Arc<dyn GraphQlTargetPlugin>> {
        self.plugins.get(name).cloned().ok_or_else(|| {
            StygianError::Config(ConfigError::MissingConfig(format!(
                "no GraphQL plugin registered for target '{name}'"
            )))
        })
    }

    /// List all registered plugin names.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::application::graphql_plugin_registry::GraphQlPluginRegistry;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    /// use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
    /// use std::collections::HashMap;
    /// use std::sync::Arc;
    ///
    /// struct P;
    /// impl GraphQlTargetPlugin for P {
    ///     fn name(&self) -> &str { "p" }
    ///     fn endpoint(&self) -> &str { "https://p.example.com/graphql" }
    /// }
    ///
    /// let mut registry = GraphQlPluginRegistry::new();
    /// registry.register(Arc::new(P));
    /// assert!(registry.list().contains(&"p"));
    /// ```
    pub fn list(&self) -> Vec<&str> {
        self.plugins.keys().map(String::as_str).collect()
    }
}

impl Default for GraphQlPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ports::graphql_plugin::GraphQlTargetPlugin;

    struct Plugin(&'static str, &'static str);

    impl GraphQlTargetPlugin for Plugin {
        fn name(&self) -> &str {
            self.0
        }
        fn endpoint(&self) -> &str {
            self.1
        }
    }

    #[test]
    fn register_and_get_plugin() {
        let mut registry = GraphQlPluginRegistry::new();
        registry.register(Arc::new(Plugin(
            "jobber",
            "https://api.getjobber.com/api/graphql",
        )));
        let plugin = registry.get("jobber").unwrap();
        assert_eq!(plugin.endpoint(), "https://api.getjobber.com/api/graphql");
    }

    #[test]
    fn get_unknown_plugin_returns_error() {
        let registry = GraphQlPluginRegistry::new();
        assert!(
            matches!(registry.get("unknown"), Err(StygianError::Config(_))),
            "expected Config error for unregistered plugin"
        );
    }

    #[test]
    fn register_overwrites_previous() {
        let mut registry = GraphQlPluginRegistry::new();
        registry.register(Arc::new(Plugin("api", "https://v1.example.com/graphql")));
        registry.register(Arc::new(Plugin("api", "https://v2.example.com/graphql")));
        let plugin = registry.get("api").unwrap();
        assert_eq!(plugin.endpoint(), "https://v2.example.com/graphql");
    }

    #[test]
    fn list_returns_all_names() {
        let mut registry = GraphQlPluginRegistry::new();
        registry.register(Arc::new(Plugin("alpha", "https://a.example.com/graphql")));
        registry.register(Arc::new(Plugin("beta", "https://b.example.com/graphql")));
        let mut names = registry.list();
        names.sort_unstable();
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
