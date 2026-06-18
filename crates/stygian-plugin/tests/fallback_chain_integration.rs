#![cfg(feature = "graph-integration")]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_const_for_fn
)]
//! Cross-crate integration tests: `FallbackChainService` + `PluginExtractionAdapter`
//!
//! These tests verify the full Phase 5 wiring:
//! - A plain HTTP primary service guarded by a circuit breaker
//! - A `PluginExtractionAdapter` as the last-resort fallback

//! - The chain correctly falls back when the primary fails
//! - The chain correctly trips the circuit after threshold failures and skips
//!   the primary on subsequent calls, routing directly to the plugin adapter
//! - Templates created via `PluginTemplateStore` are immediately available to
//!   the `PluginExtractionAdapter` in the chain (shared-store contract)

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tempfile::TempDir;

use stygian_graph::adapters::fallback::FallbackChainService;
use stygian_graph::adapters::noop::NoopService;
use stygian_graph::adapters::resilience::CircuitBreakerImpl;
use stygian_graph::domain::error::{Result, ServiceError, StygianError};
use stygian_graph::ports::{CircuitBreaker, CircuitState, ScrapingService, ServiceInput};
use stygian_plugin::{
    ExtractionTemplate, PluginTemplateStore, Region, Selector,
    adapters::{ExtractionEngine, PluginExtractionAdapter},
    storage::{FileTemplateStore, MemoryIdempotencyStore},
};

// ── Test helpers ──────────────────────────────────────────────────────────────

/// A service that always returns an error — used to simulate a downed primary.
struct AlwaysFailService {
    name: &'static str,
}

#[async_trait]
impl ScrapingService for AlwaysFailService {
    async fn execute(&self, _input: ServiceInput) -> Result<stygian_graph::ports::ServiceOutput> {
        Err(StygianError::Service(ServiceError::Unavailable(format!(
            "service '{}' is intentionally failing",
            self.name
        ))))
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// Build shared in-memory stores backed by a temp directory for templates.
fn build_stores(tmp: &TempDir) -> (Arc<FileTemplateStore>, Arc<MemoryIdempotencyStore>) {
    (
        Arc::new(FileTemplateStore::new(tmp.path().to_path_buf())),
        Arc::new(MemoryIdempotencyStore::new()),
    )
}

/// Minimal HTML that the test template can extract from.
const TEST_HTML: &str = r#"
<html>
<body>
  <h1 class="title">Hello World</h1>
  <span class="price">42.00</span>
</body>
</html>
"#;

/// Create a simple extraction template stored in `store`.
async fn seed_template(
    store: &Arc<FileTemplateStore>,
    name: &str,
) -> stygian_plugin::ExtractionTemplate {
    let template = ExtractionTemplate::new(name)
        .with_region(Region::new(
            "title",
            Selector::css(".title"),
            json!({"type": "string"}),
        ))
        .with_region(Region::new(
            "price",
            Selector::css(".price"),
            json!({"type": "string"}),
        ));
    let Ok(()) = store.save(&template).await else {
        panic!("Failed to save template");
    };
    template
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// When the primary succeeds the result is returned immediately and the plugin
/// fallback is never called.
#[tokio::test]
async fn test_primary_succeeds_no_fallback_needed() {
    let breaker = CircuitBreakerImpl::new(3, Duration::from_secs(30));
    let chain = FallbackChainService::builder()
        .add(Arc::new(NoopService), breaker)
        .named("noop-only-chain")
        .build();

    let input = ServiceInput {
        url: "https://example.com".to_string(),
        params: json!({}),
    };
    let Ok(output) = chain.execute(input).await else {
        panic!("primary should succeed");
    };
    assert_eq!(
        output
            .metadata
            .get("service")
            .and_then(serde_json::Value::as_str),
        Some("noop")
    );
    assert_eq!(
        output
            .metadata
            .get("success")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

/// When the primary service fails, the plugin extraction adapter fires as the
/// fallback and returns structured data.
#[tokio::test]
async fn test_plugin_fallback_fires_on_primary_failure()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let (template_store, idempotency_store) = build_stores(&tmp);
    let template = seed_template(&template_store, "Fallback Test Template").await;

    let plugin_fallback = Arc::new(PluginExtractionAdapter::new(
        Arc::clone(&template_store) as Arc<dyn stygian_plugin::ports::PluginTemplateStore>,
        Arc::new(ExtractionEngine),
        Arc::clone(&idempotency_store) as Arc<dyn stygian_plugin::ports::IdempotencyKeyStore>,
    ));

    let chain = FallbackChainService::builder()
        .add(
            Arc::new(AlwaysFailService {
                name: "always-fail",
            }),
            CircuitBreakerImpl::new(5, Duration::from_secs(30)),
        )
        .add(
            plugin_fallback,
            CircuitBreakerImpl::new(3, Duration::from_secs(30)),
        )
        .named("fail-to-plugin")
        .build();

    let input = ServiceInput {
        url: "https://example.com".to_string(),
        params: json!({
            "template_id": template.id.to_string(),
            "html": TEST_HTML
        }),
    };
    let output = chain.execute(input).await?;
    // The plugin adapter ran successfully — it returns a JSON data payload.
    assert!(!output.data.is_empty(), "plugin output should not be empty");
    Ok(())
}

/// After the primary trips its circuit breaker (threshold failures reached), the
/// circuit opens and subsequent calls skip the primary and route directly to the
/// plugin fallback without paying the cost of another failure attempt.
#[tokio::test]
async fn test_circuit_opens_and_primary_skipped()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let (template_store, idempotency_store) = build_stores(&tmp);
    let template = seed_template(&template_store, "Circuit Trip Template").await;

    // Use a very short reset timeout so the test can verify the skip behaviour
    // synchronously without sleeping.
    let primary_breaker = CircuitBreakerImpl::new(2, std::time::Duration::from_hours(1));

    // Trip the breaker by recording failures directly — simulates two failed calls.
    primary_breaker.record_failure();
    primary_breaker.record_failure();
    assert_eq!(
        primary_breaker.state(),
        CircuitState::Open,
        "circuit should be open after threshold failures"
    );

    let plugin_fallback = Arc::new(PluginExtractionAdapter::new(
        Arc::clone(&template_store) as Arc<dyn stygian_plugin::ports::PluginTemplateStore>,
        Arc::new(ExtractionEngine),
        Arc::clone(&idempotency_store) as Arc<dyn stygian_plugin::ports::IdempotencyKeyStore>,
    ));
    // Use a separate fallback breaker that starts closed.
    let fallback_breaker = CircuitBreakerImpl::new(3, Duration::from_secs(30));

    let chain = FallbackChainService::builder()
        .add(
            Arc::new(AlwaysFailService {
                name: "already-broken",
            }),
            // Pass the already-open breaker by value.
            primary_breaker,
        )
        .add(plugin_fallback, fallback_breaker)
        .named("pre-open-circuit-chain")
        .build();

    let input = ServiceInput {
        url: "https://example.com".to_string(),
        params: json!({
            "template_id": template.id.to_string(),
            "html": TEST_HTML
        }),
    };
    // With the primary circuit open, the chain should route directly to plugin.
    let output = chain.execute(input).await?;
    assert!(
        !output.data.is_empty(),
        "plugin fallback output must be non-empty"
    );
    Ok(())
}

/// Shared-store contract: a template saved via `PluginTemplateStore` is
/// immediately readable by the `PluginExtractionAdapter` in the fallback chain —
/// no cache invalidation or reload needed.
#[tokio::test]
async fn test_shared_store_contract() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let (template_store, idempotency_store) = build_stores(&tmp);

    // Save template BEFORE building the adapter — simulating what the MCP server
    // does when a user calls plugin_create_template.
    let template = seed_template(&template_store, "Shared Store Template").await;

    let plugin_adapter = Arc::new(PluginExtractionAdapter::new(
        Arc::clone(&template_store) as Arc<dyn stygian_plugin::ports::PluginTemplateStore>,
        Arc::new(ExtractionEngine),
        Arc::clone(&idempotency_store) as Arc<dyn stygian_plugin::ports::IdempotencyKeyStore>,
    ));
    let chain = FallbackChainService::builder()
        .add(
            Arc::new(AlwaysFailService { name: "fail" }),
            CircuitBreakerImpl::new(5, Duration::from_secs(30)),
        )
        .add(
            plugin_adapter,
            CircuitBreakerImpl::new(3, Duration::from_secs(30)),
        )
        .build();

    let input = ServiceInput {
        url: "https://shared-store-test.example".to_string(),
        params: json!({
            "template_id": template.id.to_string(),
            "html": TEST_HTML
        }),
    };
    let Ok(_) = chain.execute(input).await else {
        panic!("shared store access should succeed");
    };
    Ok(())
}

/// An empty chain returns `ServiceError::Unavailable`.
#[tokio::test]
async fn test_empty_fallback_chain_returns_error() {
    let chain = FallbackChainService::builder().build();
    let input = ServiceInput {
        url: "https://example.com".to_string(),
        params: json!({}),
    };
    let result = chain.execute(input).await;
    assert!(result.is_err(), "empty chain must return an error");
}

/// Two independent calls with the same idempotency key should produce identical
/// output (second call returns the cached result).
#[tokio::test]
async fn test_idempotent_fallback_calls() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let (template_store, idempotency_store) = build_stores(&tmp);
    let template = seed_template(&template_store, "Idempotency Template").await;

    let plugin_adapter = Arc::new(PluginExtractionAdapter::new(
        Arc::clone(&template_store) as Arc<dyn stygian_plugin::ports::PluginTemplateStore>,
        Arc::new(ExtractionEngine),
        Arc::clone(&idempotency_store) as Arc<dyn stygian_plugin::ports::IdempotencyKeyStore>,
    ));
    let chain = FallbackChainService::builder()
        .add(
            Arc::new(AlwaysFailService { name: "fail" }),
            CircuitBreakerImpl::new(5, Duration::from_secs(30)),
        )
        .add(
            plugin_adapter,
            CircuitBreakerImpl::new(3, Duration::from_secs(30)),
        )
        .build();

    let idem_key = "01JTEST0000000000000000000"; // fixed ULID for determinism
    let make_input = || ServiceInput {
        url: "https://idem-test.example".to_string(),
        params: json!({
            "template_id": template.id.to_string(),
            "html": TEST_HTML,
            "idempotency_key": idem_key
        }),
    };

    let first = chain.execute(make_input()).await?;
    let second = chain.execute(make_input()).await?;
    assert_eq!(
        first.data, second.data,
        "idempotent calls must return identical data"
    );
    Ok(())
}
