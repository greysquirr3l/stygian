//! Jobber GraphQL plugin — see T36 for the full implementation.
//!
//! Jobber is a field-service management platform whose GraphQL API lives at
//! `https://api.getjobber.com/api/graphql` and requires the version header
//! `X-JOBBER-GRAPHQL-VERSION: 2025-04-16` on every request.

use std::collections::HashMap;

use crate::ports::graphql_plugin::GraphQlTargetPlugin;
use crate::ports::{GraphQlAuth, GraphQlAuthKind};

/// Jobber GraphQL API plugin.
///
/// Supplies the endpoint, required version header, and default bearer-token
/// auth for all Jobber pipeline nodes.  The access token is read from the
/// `JOBBER_ACCESS_TOKEN` environment variable **at construction time** via
/// [`JobberPlugin::new`] (or [`Default`]), so `default_auth` performs no
/// environment access after the plugin is built.  Use [`JobberPlugin::with_token`]
/// to inject a token directly (useful in tests and programmatic usage).
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_plugins::jobber::JobberPlugin;
/// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
///
/// let plugin = JobberPlugin::new();
/// assert_eq!(plugin.name(), "jobber");
/// assert_eq!(plugin.endpoint(), "https://api.getjobber.com/api/graphql");
/// ```
pub struct JobberPlugin {
    token: Option<String>,
}

impl JobberPlugin {
    /// Creates a new [`JobberPlugin`], reading the access token from the
    /// `JOBBER_ACCESS_TOKEN` environment variable.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::jobber::JobberPlugin;
    ///
    /// let plugin = JobberPlugin::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a [`JobberPlugin`] with an explicit access token, bypassing the
    /// environment entirely.
    ///
    /// This is useful when credentials are already available at call-site (e.g.,
    /// fetched from a secret store) or when writing tests without mutating
    /// process environment variables.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::adapters::graphql_plugins::jobber::JobberPlugin;
    /// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
    ///
    /// let plugin = JobberPlugin::with_token("my-secret-token");
    /// assert!(plugin.default_auth().is_some());
    /// ```
    #[must_use]
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            token: Some(token.into()),
        }
    }
}

impl Default for JobberPlugin {
    /// Creates a [`JobberPlugin`] by reading `JOBBER_ACCESS_TOKEN` from the
    /// environment at construction time.
    fn default() -> Self {
        Self {
            token: std::env::var("JOBBER_ACCESS_TOKEN").ok(),
        }
    }
}

impl GraphQlTargetPlugin for JobberPlugin {
    fn name(&self) -> &'static str {
        "jobber"
    }

    fn endpoint(&self) -> &'static str {
        "https://api.getjobber.com/api/graphql"
    }

    fn version_headers(&self) -> HashMap<String, String> {
        [(
            "X-JOBBER-GRAPHQL-VERSION".to_string(),
            "2025-04-16".to_string(),
        )]
        .into()
    }

    fn default_auth(&self) -> Option<GraphQlAuth> {
        self.token.as_ref().map(|token| GraphQlAuth {
            kind: GraphQlAuthKind::Bearer,
            token: token.clone(),
            header_name: None,
        })
    }

    fn description(&self) -> &'static str {
        "Jobber field-service management GraphQL API"
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_is_jobber() {
        assert_eq!(JobberPlugin::new().name(), "jobber");
    }

    #[test]
    fn endpoint_is_correct() {
        assert_eq!(
            JobberPlugin::new().endpoint(),
            "https://api.getjobber.com/api/graphql"
        );
    }

    #[test]
    fn version_header_is_set() {
        let headers = JobberPlugin::new().version_headers();
        assert_eq!(
            headers.get("X-JOBBER-GRAPHQL-VERSION").map(String::as_str),
            Some("2025-04-16")
        );
    }

    #[test]
    fn default_auth_with_injected_token() {
        let plugin = JobberPlugin::with_token("test-token-abc");
        let auth = plugin.default_auth();
        assert!(auth.is_some(), "auth should be Some when token is injected");
        let auth = auth.expect("auth should be Some when token is injected");
        assert_eq!(auth.kind, GraphQlAuthKind::Bearer);
        assert_eq!(auth.token, "test-token-abc");
        assert!(auth.header_name.is_none());
    }

    #[test]
    fn default_auth_absent_when_no_token() {
        let plugin = JobberPlugin { token: None };
        assert!(plugin.default_auth().is_none());
    }

    #[test]
    fn description_is_non_empty() {
        assert!(!JobberPlugin::new().description().is_empty());
    }
}
