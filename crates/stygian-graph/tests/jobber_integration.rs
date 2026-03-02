//! Integration tests for the Jobber GraphQL plugin.
//!
//! These tests hit the live Jobber API and require `JOBBER_ACCESS_TOKEN` to be
//! set in the environment.  They are intentionally `#[ignore]`-gated so they
//! never run in CI unless explicitly invoked:
//!
//! ```bash
//! JOBBER_ACCESS_TOKEN=<token> cargo test --test jobber_integration -- --ignored
//! ```

#![allow(clippy::expect_used, clippy::needless_raw_string_hashes)]

use stygian_graph::adapters::graphql_plugins::jobber::JobberPlugin;
use stygian_graph::ports::graphql_plugin::GraphQlTargetPlugin;

/// Smoke-test the plugin's static metadata without any network calls.
#[test]
fn jobber_plugin_metadata() {
    let plugin = JobberPlugin::new();
    assert_eq!(plugin.name(), "jobber");
    assert_eq!(plugin.endpoint(), "https://api.getjobber.com/api/graphql");

    let headers = plugin.version_headers();
    assert_eq!(
        headers.get("X-JOBBER-GRAPHQL-VERSION").map(String::as_str),
        Some("2025-04-16")
    );
    assert!(plugin.supports_cursor_pagination());
    assert_eq!(plugin.default_page_size(), 50);
    assert!(!plugin.description().is_empty());
}

/// Verify that the clients query reaches the live Jobber API and returns data.
///
/// Run with:
/// ```bash
/// JOBBER_ACCESS_TOKEN=<token> cargo test --test jobber_integration -- --ignored
/// ```
#[tokio::test]
#[ignore = "requires JOBBER_ACCESS_TOKEN env var"]
async fn test_jobber_clients_returns_data() {
    let token = std::env::var("JOBBER_ACCESS_TOKEN")
        .expect("JOBBER_ACCESS_TOKEN must be set to run integration tests");

    let client = reqwest::Client::new();

    let query = serde_json::json!({
        "operationName": "ListClients",
        "query": r#"
            query ListClients($first: Int) {
              clients(first: $first) {
                edges {
                  node { id name }
                }
                pageInfo { hasNextPage endCursor }
              }
            }
        "#,
        "variables": { "first": 5 }
    });

    let response = client
        .post(JobberPlugin::new().endpoint())
        .bearer_auth(&token)
        .header("X-JOBBER-GRAPHQL-VERSION", "2025-04-16")
        .header("Content-Type", "application/json")
        .json(&query)
        .send()
        .await
        .expect("HTTP request should succeed");

    assert!(
        response.status().is_success(),
        "Expected 200 OK, got {}",
        response.status()
    );

    let body: serde_json::Value = response
        .json()
        .await
        .expect("Response should be valid JSON");

    // Assert we got a data payload, not just errors
    assert!(
        body.get("errors").is_none(),
        "Unexpected GraphQL errors: {body:#?}"
    );

    let edges = body
        .pointer("/data/clients/edges")
        .expect("Response must contain data.clients.edges");

    assert!(
        edges.as_array().is_some_and(|a| !a.is_empty()),
        "Expected at least one client edge, got: {edges:#?}"
    );
}
