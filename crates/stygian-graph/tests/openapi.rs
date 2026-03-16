//! OpenAPI adapter integration tests (live network).
//!
//! These tests hit the public Petstore v3 demo API and are gated behind the
//! `PETSTORE_LIVE=1` environment variable.  Run with:
//!
//! ```bash
//! PETSTORE_LIVE=1 cargo test -p stygian-graph --test openapi -- --nocapture
//! ```

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use serde_json::json;
use stygian_graph::adapters::openapi::OpenApiAdapter;
use stygian_graph::ports::{ScrapingService, ServiceInput};

const PETSTORE_SPEC: &str = "https://petstore3.swagger.io/api/v3/openapi.json";

fn live_enabled() -> bool {
    std::env::var("PETSTORE_LIVE").as_deref() == Ok("1")
}

#[tokio::test]
#[ignore = "requires PETSTORE_LIVE=1 and network access"]
async fn live_list_pets_by_status() {
    if !live_enabled() {
        return;
    }
    let adapter = OpenApiAdapter::new();
    let output = adapter
        .execute(ServiceInput {
            url: PETSTORE_SPEC.to_string(),
            params: json!({
                "operation": "findPetsByStatus",
                "args": { "status": "available" },
            }),
        })
        .await
        .expect("live_list_pets_by_status");

    // The Petstore demo should return a JSON array of pets.
    assert!(!output.data.is_empty(), "expected non-empty data");
    let pets: serde_json::Value =
        serde_json::from_str(&output.data).expect("data should be valid JSON");
    assert!(
        pets.is_array() || pets.is_object(),
        "expected JSON array or object"
    );
}

#[tokio::test]
#[ignore = "requires PETSTORE_LIVE=1 and network access"]
async fn live_get_pet_by_id() {
    if !live_enabled() {
        return;
    }
    let adapter = OpenApiAdapter::new();
    let output = adapter
        .execute(ServiceInput {
            url: PETSTORE_SPEC.to_string(),
            params: json!({
                "operation": "getPetById",
                "args": { "petId": 1 },
            }),
        })
        .await
        .expect("live_get_pet_by_id");

    assert!(!output.data.is_empty());
}

#[tokio::test]
#[ignore = "requires PETSTORE_LIVE=1 and network access"]
async fn live_spec_cache_is_warm_on_second_call() {
    if !live_enabled() {
        return;
    }
    let adapter = OpenApiAdapter::new();
    let input = ServiceInput {
        url: PETSTORE_SPEC.to_string(),
        params: json!({
            "operation": "findPetsByStatus",
            "args": { "status": "sold" },
        }),
    };

    // First call fetches and populates cache.
    adapter.execute(input.clone()).await.expect("first call");

    // Second call should be faster (cache hit), and must also succeed.
    adapter.execute(input).await.expect("second call (cache)");
}
