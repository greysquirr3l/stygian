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
        std::env::var("JOBBER_ACCESS_TOKEN")
            .ok()
            .map(|token| GraphQlAuth {
                kind: GraphQlAuthKind::Bearer,
                token,
                header_name: None,
            })
    }

    fn description(&self) -> &'static str {
        "Jobber field-service management GraphQL API"
    }
}

#[cfg(test)]
#[allow(unsafe_code, clippy::expect_used)] // set_var/remove_var are unsafe in Rust ≥1.93; scoped to tests only
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env-var mutations so parallel test threads don't race each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let key = "JOBBER_ACCESS_TOKEN";
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::var(key).ok();
        // SAFETY: ENV_LOCK serialises all env mutations in this module
        unsafe { std::env::set_var(key, "test-token-abc") };

        let auth = JobberPlugin.default_auth();
        assert!(auth.is_some(), "auth should be Some when env var is set");
        let auth = auth.expect("auth should be Some when env var is set");
        assert_eq!(auth.kind, GraphQlAuthKind::Bearer);
        assert_eq!(auth.token, "test-token-abc");
        assert!(auth.header_name.is_none());

        // Restore previous state
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    #[test]
    fn default_auth_absent_when_no_env() {
        let key = "JOBBER_ACCESS_TOKEN";
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::var(key).ok();
        // SAFETY: ENV_LOCK serialises all env mutations in this module
        unsafe { std::env::remove_var(key) };

        assert!(JobberPlugin.default_auth().is_none());

        // Restore
        if let Some(v) = prev {
            unsafe { std::env::set_var(key, v) };
        }
    }

    #[test]
    fn description_is_non_empty() {
        assert!(!JobberPlugin.description().is_empty());
    }
}
