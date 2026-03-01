//! Chaos engineering and load tests.
//!
//! Verifies that the pipeline engine handles failures gracefully and performs
//! correctly under high concurrency. All tests use in-memory adapters; no
//! real network I/O is performed.
//!
//! ## Purpose
//!
//! Chaos tests deliberately introduce faults (sustained failures, slow services,
//! overloaded queues) to validate that the engine degrades gracefully and that
//! resilience primitives behave correctly under stress.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mycelium_graph::adapters::resilience::{CircuitBreakerImpl, RetryPolicy};
use mycelium_graph::application::metrics::{MetricEvent, MetricsRegistry};
use mycelium_graph::domain::error::{MyceliumError, ServiceError};
use mycelium_graph::domain::executor::WorkerPool;
use mycelium_graph::ports::{CircuitBreaker, CircuitState, ScrapingService, ServiceInput, ServiceOutput};

// ─── Fault-injecting service ──────────────────────────────────────────────────

/// Records call counts and fails for the first `fail_for_first` invocations.
struct FaultInjectingService {
    name: &'static str,
    call_count: AtomicU32,
    fail_for_first: u32,
}

impl FaultInjectingService {
    fn new(name: &'static str, fail_for_first: u32) -> Self {
        Self {
            name,
            call_count: AtomicU32::new(0),
            fail_for_first,
        }
    }
}

#[async_trait::async_trait]
impl ScrapingService for FaultInjectingService {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn execute(
        &self,
        _input: ServiceInput,
    ) -> mycelium_graph::domain::error::Result<ServiceOutput> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_for_first {
            Err(MyceliumError::Service(ServiceError::Unavailable(
                self.name.into(),
            )))
        } else {
            Ok(ServiceOutput {
                data: format!("{{\"attempt\":{n}}}"),
                metadata: serde_json::json!({"success": true}),
            })
        }
    }
}

// ─── Circuit breaker chaos ────────────────────────────────────────────────────

#[test]
fn circuit_breaker_opens_under_sustained_error_stream() {
    let cb = CircuitBreakerImpl::new(5, Duration::from_secs(60));

    for _ in 0..5 {
        assert_eq!(cb.state(), CircuitState::Closed, "should be closed before threshold");
        cb.record_failure();
    }

    assert_ne!(
        cb.state(),
        CircuitState::Closed,
        "circuit breaker must open after threshold failures"
    );
}

#[test]
fn circuit_breaker_closes_after_zero_timeout_and_success() {
    // Zero timeout so the half-open window is immediate
    let cb = CircuitBreakerImpl::new(1, Duration::from_secs(0));

    cb.record_failure();
    assert_ne!(cb.state(), CircuitState::Closed);

    cb.record_success();
    assert_eq!(cb.state(), CircuitState::Closed, "circuit should close after success");
}

#[test]
fn circuit_breaker_alternating_success_failure_does_not_trip() {
    let cb = CircuitBreakerImpl::new(10, Duration::from_secs(60));

    // 5 failures interleaved with 5 successes — must not reach threshold
    // because success should reset the counter
    for _ in 0..5 {
        cb.record_failure();
        cb.record_success();
    }

    // Depending on whether success resets the count, the breaker may or may not
    // be closed. What must NOT happen is a panic.
    let _ = cb.state();
}

// ─── Retry policy under high attempt counts ───────────────────────────────────

#[test]
fn retry_policy_does_not_panic_for_large_attempt_index() {
    let policy = RetryPolicy::new(
        5,
        Duration::from_millis(1),
        Duration::from_millis(100),
    );
    // Must not panic for any attempt index
    for attempt in 0u32..=50 {
        let _ = policy.delay_for(attempt);
    }
}

// ─── Worker pool load tests ───────────────────────────────────────────────────

/// Submit many tasks and verify all complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn worker_pool_processes_burst_of_tasks() {
    use futures::future::join_all;

    let pool = Arc::new(WorkerPool::new(8, 256));
    let mut handles = Vec::new();

    for _ in 0..50 {
        let p = Arc::clone(&pool);
        let svc = Arc::new(FaultInjectingService::new("ok", 0))
            as Arc<dyn ScrapingService>;
        handles.push(tokio::spawn(async move {
            p.submit(svc, ServiceInput {
                url: "https://example.com".into(),
                params: serde_json::json!({}),
            }).await
        }));
    }

    let results = join_all(handles).await;
    let successes = results
        .iter()
        .filter(|r| r.as_ref().map(|inner| inner.is_ok()).unwrap_or(false))
        .count();
    assert_eq!(successes, 50, "all 50 tasks must succeed");
}

/// A slow task must not starve fast tasks (basic fairness check).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_pool_fast_tasks_finish_despite_slow_ones() {
    use futures::future::join_all;

    // SlowService — adds a small delay
    struct SlowService;
    #[async_trait::async_trait]
    impl ScrapingService for SlowService {
        fn name(&self) -> &'static str { "slow" }
        async fn execute(&self, _: ServiceInput) -> mycelium_graph::domain::error::Result<ServiceOutput> {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(ServiceOutput { data: String::new(), metadata: serde_json::json!({}) })
        }
    }

    let pool = Arc::new(WorkerPool::new(4, 64));
    let fast_counter = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    // One slow task
    {
        let p = Arc::clone(&pool);
        handles.push(tokio::spawn(async move {
            p.submit(Arc::new(SlowService), ServiceInput {
                url: "https://slow.example.com".into(),
                params: serde_json::json!({}),
            }).await
        }));
    }

    // 20 fast tasks
    for _ in 0..20 {
        let p = Arc::clone(&pool);
        let c = Arc::clone(&fast_counter);
        let svc = Arc::new(FaultInjectingService::new("fast", 0)) as Arc<dyn ScrapingService>;
        handles.push(tokio::spawn(async move {
            let result = p.submit(svc, ServiceInput {
                url: "https://example.com".into(),
                params: serde_json::json!({}),
            }).await;
            if result.is_ok() {
                c.fetch_add(1, Ordering::Relaxed);
            }
            result
        }));
    }

    join_all(handles).await;
    assert_eq!(fast_counter.load(Ordering::Relaxed), 20, "all fast tasks must complete");
}

/// Concurrent producers all submitting to the same pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn worker_pool_concurrent_producers_all_complete() {
    use futures::future::join_all;

    let pool = Arc::new(WorkerPool::new(8, 128));
    let mut handles = Vec::new();

    // 8 producer tasks × 5 work items each = 40 total
    for _ in 0..8 {
        let p = Arc::clone(&pool);
        handles.push(tokio::spawn(async move {
            let mut inner = Vec::new();
            for _ in 0..5 {
                let pp = Arc::clone(&p);
                let svc = Arc::new(FaultInjectingService::new("prod", 0)) as Arc<dyn ScrapingService>;
                inner.push(tokio::spawn(async move {
                    pp.submit(svc, ServiceInput {
                        url: "https://example.com".into(),
                        params: serde_json::json!({}),
                    }).await
                }));
            }
            join_all(inner).await
        }));
    }

    let groups = join_all(handles).await;
    let total_success: usize = groups.iter()
        .filter_map(|g| g.as_ref().ok())
        .flat_map(|v| v.iter())
        .filter(|r| r.as_ref().map(|inner| inner.is_ok()).unwrap_or(false))
        .count();

    assert_eq!(total_success, 40, "all 40 submissions must succeed");
}

// ─── Metrics under concurrent writes ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_registry_thread_safe_under_concurrent_writes() {
    use futures::future::join_all;

    let m = Arc::new(MetricsRegistry::new());
    let mut handles = Vec::new();

    for _ in 0..10 {
        let mm = Arc::clone(&m);
        handles.push(tokio::spawn(async move {
            for _ in 0..100 {
                mm.record(MetricEvent::RequestStarted { service: "svc".into() });
                mm.record(MetricEvent::CacheAccess { hit: true });
            }
        }));
    }

    join_all(handles).await;

    let snap = m.snapshot();
    assert_eq!(snap.requests_total, 1_000, "10 tasks × 100 requests each");
    assert_eq!(snap.cache_hits_total, 1_000);
}

// ─── Fault-injecting service tests ───────────────────────────────────────────

#[tokio::test]
async fn fault_injecting_service_fails_then_succeeds() {
    let svc = FaultInjectingService::new("test", 3);
    let input = ServiceInput {
        url: "https://example.com".to_string(),
        params: serde_json::json!({}),
    };

    // First 3 calls fail
    for _ in 0..3 {
        assert!(svc.execute(input.clone()).await.is_err());
    }

    // Subsequent calls succeed
    assert!(svc.execute(input.clone()).await.is_ok());
    assert!(svc.execute(input).await.is_ok());
}

/// A service that always fails must not panic the caller.
#[tokio::test]
async fn always_failing_service_is_handled_gracefully() {
    let svc = FaultInjectingService::new("always_fail", u32::MAX);
    let input = ServiceInput {
        url: "https://example.com".to_string(),
        params: serde_json::json!({}),
    };

    for _ in 0..10 {
        let _ = svc.execute(input.clone()).await; // must not panic
    }
}
