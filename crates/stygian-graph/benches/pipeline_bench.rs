//! Criterion benchmarks for stygian-graph core hot paths.
//!
//! Run with: `cargo bench -p stygian-graph`

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::semicolon_if_nothing_returned,
    clippy::doc_markdown
)]

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde_json::json;
use stygian_graph::adapters::cache::{BoundedLruCache, DashMapCache};
use stygian_graph::adapters::noop::NoopService;
use stygian_graph::application::metrics::{MetricEvent, MetricsRegistry};
use stygian_graph::application::registry::ServiceRegistry;
use stygian_graph::domain::graph::{DagExecutor, Edge, Node, Pipeline};
use stygian_graph::ports::{CachePort, ScrapingService};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn noop_services() -> HashMap<String, Arc<dyn ScrapingService>> {
    let mut m = HashMap::new();
    m.insert(
        "noop".to_string(),
        Arc::new(NoopService) as Arc<dyn ScrapingService>,
    );
    m
}

fn linear_pipeline(n: usize) -> Pipeline {
    let mut p = Pipeline::new("bench");
    for i in 0..n {
        let node_id = format!("n{i}");
        p.add_node(Node::new(node_id, "noop", json!({})));
    }
    for i in 1..n {
        let from = format!("n{}", i - 1);
        let to = format!("n{i}");
        p.add_edge(Edge::new(from, to));
    }
    p
}

// ─── DAG execution benchmarks ─────────────────────────────────────────────────

/// Measures DAG construction overhead for pipelines of various sizes.
fn bench_dag_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag_construction");
    for n in [1, 5, 10, 50, 100] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| DagExecutor::from_pipeline(&linear_pipeline(n)));
        });
    }
    group.finish();
}

/// Measures full end-to-end pipeline execution (build + run).
fn bench_dag_execution(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio");
    let services = noop_services();

    let mut group = c.benchmark_group("dag_execution");
    for n in [1, 5, 10, 25] {
        let p = linear_pipeline(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let ex = DagExecutor::from_pipeline(&p).expect("build");
                ex.execute(&services).await.expect("execute")
            });
        });
    }
    group.finish();
}

// ─── Cache throughput benchmark ───────────────────────────────────────────────

/// Measures read-heavy LRU cache throughput (90% reads, 10% writes).
fn bench_lru_cache_throughput(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio");

    c.bench_function("lru_cache_set_get_100_keys", |b| {
        b.to_async(&rt).iter(|| async {
            let cache = BoundedLruCache::new(NonZeroUsize::new(128).unwrap());
            for i in 0u32..100 {
                let key = format!("key:{i}");
                let val = format!("val:{i}");
                cache.set(&key, val, None).await.expect("set");
            }
            for i in 0u32..100 {
                let key = format!("key:{i}");
                cache.get(&key).await.expect("get");
            }
        });
    });
}

/// Measures DashMap cache throughput (write-heavy scenario).
fn bench_dashmap_cache_write_heavy(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio");

    c.bench_function("dashmap_cache_write_heavy_200_entries", |b| {
        b.to_async(&rt).iter(|| async {
            let cache = DashMapCache::new(Duration::from_mins(5));
            for i in 0u32..200 {
                let key = format!("k{i}");
                let val = format!("v{i}");
                cache.set(&key, val, None).await.expect("set");
            }
        });
    });
}

// ─── ServiceRegistry benchmark ────────────────────────────────────────────────

/// Measures registry lookup cost with varying number of registered services.
fn bench_registry_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("registry_lookup");
    for n in [1, 10, 100] {
        let registry = ServiceRegistry::new();
        for i in 0..n {
            let service_name = format!("svc_{i}");
            registry.register(
                service_name,
                Arc::new(NoopService) as Arc<dyn ScrapingService>,
            );
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let lookup_name = format!("svc_{}", n - 1);
            b.iter(|| registry.get(&lookup_name));
        });
    }
    group.finish();
}

// ─── Metrics recording benchmark ──────────────────────────────────────────────

/// Measures the cost of recording metrics under single-threaded load.
fn bench_metrics_record(c: &mut Criterion) {
    c.bench_function("metrics_record_1000_events", |b| {
        b.iter(|| {
            let m = MetricsRegistry::new();
            for _ in 0..1_000 {
                m.record(MetricEvent::RequestStarted {
                    service: "svc".into(),
                });
            }
            m.snapshot()
        });
    });
}

// ─── Groups ───────────────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_dag_construction,
    bench_dag_execution,
    bench_lru_cache_throughput,
    bench_dashmap_cache_write_heavy,
    bench_registry_lookup,
    bench_metrics_record,
);
criterion_main!(benches);
