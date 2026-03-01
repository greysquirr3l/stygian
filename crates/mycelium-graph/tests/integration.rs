//! Integration tests — full pipeline execution with in-memory adapters.
//!
//! These tests wire together the domain, ports, and adapters without any real I/O.
//! They use `NoopService` and in-memory cache adapters to execute realistic
//! pipeline shapes end-to-end without external dependencies.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::redundant_closure_for_method_calls
)]

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use mycelium_graph::adapters::cache::{BoundedLruCache, DashMapCache};
use mycelium_graph::adapters::noop::NoopService;
use mycelium_graph::adapters::resilience::{
    CircuitBreakerImpl, NoopCircuitBreaker, NoopRateLimiter, RetryPolicy,
};
use mycelium_graph::application::health::{HealthReporter, HealthStatus};
use mycelium_graph::application::metrics::{MetricEvent, MetricsRegistry};
use mycelium_graph::application::registry::ServiceRegistry;
use mycelium_graph::domain::executor::WorkerPool;
use mycelium_graph::domain::graph::{DagExecutor, Edge, Node, Pipeline};
use mycelium_graph::ports::{
    CachePort, CircuitBreaker, CircuitState, RateLimiter, ScrapingService, ServiceInput,
};
use serde_json::json;

// ─── Helper types ─────────────────────────────────────────────────────────────

type Services = HashMap<String, Arc<dyn ScrapingService>>;

fn noop_services() -> Services {
    let mut m = Services::new();
    m.insert(
        "noop".to_string(),
        Arc::new(NoopService) as Arc<dyn ScrapingService>,
    );
    m
}

fn noop_node(id: &str) -> Node {
    Node::new(id, "noop", json!({"url": "https://example.com"}))
}

fn linear_pipeline(count: usize) -> Pipeline {
    let mut p = Pipeline::new("linear");
    for i in 0..count {
        let node_id = format!("n{i}");
        p.add_node(Node::new(node_id, "noop", json!({})));
    }
    for i in 1..count {
        let from = format!("n{}", i - 1);
        let to = format!("n{i}");
        p.add_edge(Edge::new(from, to));
    }
    p
}

// ─── Pipeline shape tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn single_node_pipeline_executes() {
    let mut p = Pipeline::new("single");
    p.add_node(noop_node("fetch"));

    let executor = DagExecutor::from_pipeline(&p).expect("build executor");
    let results = executor.execute(&noop_services()).await.expect("execute");

    assert_eq!(results.len(), 1);
    assert!(results.iter().any(|r| r.node_id == "fetch"));
}

#[tokio::test]
async fn linear_two_node_pipeline_runs_sequential() {
    let mut p = Pipeline::new("linear");
    p.add_node(noop_node("a"));
    p.add_node(noop_node("b"));
    p.add_edge(Edge::new("a", "b"));

    let executor = DagExecutor::from_pipeline(&p).expect("build");
    let results = executor.execute(&noop_services()).await.expect("execute");
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn diamond_graph_executes_all_four_nodes() {
    // top → left, top → right, left → bottom, right → bottom
    let mut p = Pipeline::new("diamond");
    for id in &["top", "left", "right", "bottom"] {
        p.add_node(noop_node(id));
    }
    p.add_edge(Edge::new("top", "left"));
    p.add_edge(Edge::new("top", "right"));
    p.add_edge(Edge::new("left", "bottom"));
    p.add_edge(Edge::new("right", "bottom"));

    let executor = DagExecutor::from_pipeline(&p).expect("build");
    let results = executor.execute(&noop_services()).await.expect("execute");
    assert_eq!(results.len(), 4);
}

#[tokio::test]
async fn pipeline_with_cycle_fails_at_construction() {
    let mut p = Pipeline::new("cyclic");
    p.add_node(noop_node("a"));
    p.add_node(noop_node("b"));
    p.add_edge(Edge::new("a", "b"));
    p.add_edge(Edge::new("b", "a")); // cycle

    assert!(DagExecutor::from_pipeline(&p).is_err());
}

#[tokio::test]
async fn missing_service_returns_error() {
    let mut p = Pipeline::new("missing");
    p.add_node(Node::new("n1", "does_not_exist", json!({})));

    let executor = DagExecutor::from_pipeline(&p).expect("build");
    let result = executor.execute(&HashMap::new()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ten_node_linear_pipeline_executes() {
    let p = linear_pipeline(10);
    let executor = DagExecutor::from_pipeline(&p).expect("build");
    let results = executor.execute(&noop_services()).await.expect("execute");
    assert_eq!(results.len(), 10);
}

// ─── ETL fixture ─────────────────────────────────────────────────────────────

fn etl_pipeline() -> Pipeline {
    let mut p = Pipeline::new("etl");
    p.add_node(Node::new("extract", "noop", json!({"stage": "extract"})));
    p.add_node(Node::new(
        "transform",
        "noop",
        json!({"stage": "transform"}),
    ));
    p.add_node(Node::new("load", "noop", json!({"stage": "load"})));
    p.add_edge(Edge::new("extract", "transform"));
    p.add_edge(Edge::new("transform", "load"));
    p
}

#[tokio::test]
async fn etl_pipeline_executes_all_three_stages() {
    let pipeline = etl_pipeline();
    let executor = DagExecutor::from_pipeline(&pipeline).expect("build");
    let results = executor
        .execute(&noop_services())
        .await
        .expect("execute etl");
    assert_eq!(results.len(), 3);
    for stage in &["extract", "transform", "load"] {
        assert!(
            results.iter().any(|r| r.node_id == *stage),
            "missing stage: {stage}"
        );
    }
}

// ─── ServiceRegistry tests ────────────────────────────────────────────────────

#[test]
fn registry_register_and_get_returns_service() {
    let registry = ServiceRegistry::new();
    registry.register("noop".into(), Arc::new(NoopService));
    assert!(registry.get("noop").is_some());
}

#[test]
fn registry_get_unknown_service_returns_none() {
    let registry = ServiceRegistry::new();
    assert!(registry.get("ghost").is_none());
}

#[test]
fn registry_names_returns_all_registered() {
    let registry = ServiceRegistry::new();
    registry.register("alpha".into(), Arc::new(NoopService));
    registry.register("beta".into(), Arc::new(NoopService));
    let mut names = registry.names();
    names.sort();
    assert_eq!(names, vec!["alpha", "beta"]);
}

#[test]
fn registry_deregister_removes_service() {
    let registry = ServiceRegistry::new();
    registry.register("temp".into(), Arc::new(NoopService));
    assert!(registry.get("temp").is_some());
    registry.deregister("temp");
    assert!(registry.get("temp").is_none());
}

// ─── Cache lifecycle tests ────────────────────────────────────────────────────

#[tokio::test]
async fn dashmap_cache_set_get_delete_lifecycle() {
    let cache = DashMapCache::new(Duration::from_secs(300));

    cache.set("key", "value".into(), None).await.expect("set");
    let val = cache.get("key").await.expect("get");
    assert_eq!(val, Some("value".into()));

    cache.invalidate("key").await.expect("invalidate");
    let gone = cache.get("key").await.expect("get after delete");
    assert!(gone.is_none());
}

#[tokio::test]
async fn dashmap_cache_exists_returns_correct_bool() {
    let cache = DashMapCache::new(Duration::from_secs(300));

    assert!(!cache.exists("missing").await.expect("exists"));
    cache.set("present", "v".into(), None).await.expect("set");
    assert!(cache.exists("present").await.expect("exists"));
}

#[tokio::test]
async fn lru_cache_evicts_beyond_capacity() {
    let cache = BoundedLruCache::new(NonZeroUsize::new(2).unwrap());

    cache.set("a", "1".into(), None).await.expect("set a");
    cache.set("b", "2".into(), None).await.expect("set b");
    cache.set("c", "3".into(), None).await.expect("set c"); // evicts "a"

    let a = cache.get("a").await.expect("get a");
    assert!(a.is_none(), "LRU should have evicted a");
    let c = cache.get("c").await.expect("get c");
    assert_eq!(c, Some("3".into()));
}

// ─── Circuit breaker tests ────────────────────────────────────────────────────

#[test]
fn circuit_breaker_opens_after_threshold_failures() {
    let cb = CircuitBreakerImpl::new(3, Duration::from_secs(60));
    assert_eq!(cb.state(), CircuitState::Closed);

    cb.record_failure();
    cb.record_failure();
    cb.record_failure();

    assert_ne!(
        cb.state(),
        CircuitState::Closed,
        "should be open after 3 failures"
    );
}

#[test]
fn circuit_breaker_closes_after_success() {
    let cb = CircuitBreakerImpl::new(1, Duration::from_secs(0));
    cb.record_failure();
    assert_ne!(cb.state(), CircuitState::Closed);

    cb.record_success();
    assert_eq!(
        cb.state(),
        CircuitState::Closed,
        "success resets the breaker"
    );
}

#[test]
fn noop_circuit_breaker_always_closed() {
    let cb = NoopCircuitBreaker;
    cb.record_failure();
    cb.record_failure();
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[tokio::test]
async fn noop_rate_limiter_always_allows() {
    let rl = NoopRateLimiter;
    for _ in 0..10 {
        assert!(rl.check_rate_limit("any").await.expect("rate limit check"));
    }
}

// ─── RetryPolicy tests ────────────────────────────────────────────────────────

#[test]
fn retry_policy_delays_increase_then_cap() {
    let policy = RetryPolicy::new(5, Duration::from_millis(100), Duration::from_millis(500))
        .with_jitter_ms(0);

    let d0 = policy.delay_for(0);
    let d1 = policy.delay_for(1);
    let d_big = policy.delay_for(20);

    assert!(d1 >= d0, "delay should grow: d0={d0:?} d1={d1:?}");
    assert!(
        d_big <= Duration::from_millis(500),
        "delay capped at max: d_big={d_big:?}"
    );
}

// ─── Metrics + Health composition ────────────────────────────────────────────

#[test]
fn metrics_records_are_visible_in_snapshot() {
    let m = MetricsRegistry::new();
    m.record(MetricEvent::RequestStarted {
        service: "http".into(),
    });
    m.record(MetricEvent::RequestCompleted {
        service: "http".into(),
        duration_ms: 80,
        success: true,
    });
    m.record(MetricEvent::CacheAccess { hit: true });

    let snap = m.snapshot();
    assert_eq!(snap.requests_total, 1);
    assert_eq!(snap.errors_total, 0);
    assert_eq!(snap.cache_hits_total, 1);
}

#[test]
fn health_reporter_composition() {
    let health = HealthReporter::new();
    health.register("http_pool", HealthStatus::Healthy);
    health.register("cache", HealthStatus::Healthy);

    let report = health.report();
    assert!(report.is_ready());
    assert!(report.is_live());
}

#[test]
fn prometheus_text_contains_expected_metric_families() {
    let m = MetricsRegistry::new();
    m.record(MetricEvent::RequestStarted {
        service: "svc".into(),
    });
    m.record(MetricEvent::PipelineExecuted {
        pipeline_id: "p".into(),
        duration_ms: 300,
        success: true,
    });

    let text = m.render_prometheus();
    for expected in &[
        "mycelium_requests_total",
        "mycelium_errors_total",
        "mycelium_pipelines_total",
        "mycelium_active_workers",
        "mycelium_queue_depth",
    ] {
        assert!(
            text.contains(expected),
            "prometheus text missing: {expected}"
        );
    }
}

// ─── Node / Edge domain validation ───────────────────────────────────────────

#[test]
fn node_with_empty_id_fails_validation() {
    assert!(Node::new("", "noop", json!({})).validate().is_err());
}

#[test]
fn node_with_empty_service_fails_validation() {
    assert!(Node::new("id", "", json!({})).validate().is_err());
}

#[test]
fn edge_with_empty_from_fails_validation() {
    assert!(Edge::new("", "b").validate().is_err());
}

#[test]
fn valid_pipeline_with_ten_nodes_passes_validation() {
    assert!(linear_pipeline(10).validate().is_ok());
}

// ─── WorkerPool scraping-focused tests ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_pool_submits_noop_service_successfully() {
    let pool = WorkerPool::new(4, 32);
    let svc = Arc::new(NoopService) as Arc<dyn ScrapingService>;
    let input = ServiceInput {
        url: "https://example.com".into(),
        params: json!({}),
    };

    let output = pool.submit(svc, input).await.expect("submit");
    assert!(output.metadata["success"].as_bool().unwrap_or(false));

    pool.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_pool_handles_concurrent_submissions() {
    use futures::future::join_all;

    let pool = Arc::new(WorkerPool::new(4, 64));
    let mut handles = Vec::new();

    for _ in 0..20 {
        let p = Arc::clone(&pool);
        let svc = Arc::new(NoopService) as Arc<dyn ScrapingService>;
        let input = ServiceInput {
            url: "https://example.com".into(),
            params: json!({}),
        };
        handles.push(tokio::spawn(async move { p.submit(svc, input).await }));
    }

    let results = join_all(handles).await;
    let successes = results
        .iter()
        .filter(|r| r.as_ref().map(Result::is_ok).unwrap_or(false))
        .count();
    assert_eq!(successes, 20, "all 20 submissions must succeed");
}
