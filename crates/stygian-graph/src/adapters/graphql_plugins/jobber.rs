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
/// auth drawn from `JOBBER_ACCESS_TOKEN` for all Jobber pipeline nodes.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::graphql_plugins::jobber::JobberPlugin;
/// use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;
///
/// let plugin = JobberPlugin;
/// assert_eq!(plugin.name(), "jobber");
/// assert_eq!(plugin.endpoint(), "https://api.getjobber.com/api/graphql");
/// ```
pub struct JobberPlugin;

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
        auth_from_env_lookup(|key| std::env::var(key).ok())
    }

    fn description(&self) -> &'static str {
        "Jobber field-service management GraphQL API"
    }
}

/// Build auth from an injectable env-lookup closure.
///
/// Extracted so tests can supply a synthetic environment without mutating the
/// process environment (which would require `unsafe` and a process-wide lock).
fn auth_from_env_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Option<GraphQlAuth> {
    lookup("JOBBER_ACCESS_TOKEN").map(|token| GraphQlAuth {
        kind: GraphQlAuthKind::Bearer,
        token,
        header_name: None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_is_jobber() {
        assert_eq!(JobberPlugin.name(), "jobber");
    }

    #[test]
    fn endpoint_is_correct() {
        assert_eq!(
            JobberPlugin.endpoint(),
            "https://api.getjobber.com/api/graphql"
        );
    }

    #[test]
    fn version_header_is_set() {
        let headers = JobberPlugin.version_headers();
        assert_eq!(
            headers.get("X-JOBBER-GRAPHQL-VERSION").map(String::as_str),
            Some("2025-04-16")
        );
    }

    #[test]
    fn default_auth_reads_env() {
        // Use injectable lookup — no process-env mutation required.
        let auth = auth_from_env_lookup(|key| {
            if key == "JOBBER_ACCESS_TOKEN" {
                Some("test-token-abc".to_string())
            } else {
                None
            }
        });
        assert!(auth.is_some(), "auth should be Some when env var is set");
        let auth = auth.expect("auth should be Some when env var is set");
        assert_eq!(auth.kind, GraphQlAuthKind::Bearer);
        assert_eq!(auth.token, "test-token-abc");
        assert!(auth.header_name.is_none());
    }

    #[test]
    fn default_auth_absent_when_no_env() {
        // Use injectable lookup — no process-env mutation required.
        let auth = auth_from_env_lookup(|_| None);
        assert!(auth.is_none());
    }

    #[test]
    fn description_is_non_empty() {
        assert!(!JobberPlugin.description().is_empty());
    }
}
