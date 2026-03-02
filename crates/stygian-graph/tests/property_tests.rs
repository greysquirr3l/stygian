//! Property-based tests using proptest.
//!
//! Verifies invariants that must hold for ALL valid inputs, not just
//! specific examples. Good for finding edge cases in graph construction,
//! cache behaviour, and metric calculations.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::manual_range_contains
)]

use std::num::NonZeroUsize;
use std::time::Duration;

use stygian_graph::adapters::cache::BoundedLruCache;
use stygian_graph::adapters::resilience::RetryPolicy;
use stygian_graph::application::metrics::{MetricEvent, MetricsRegistry};
use stygian_graph::domain::graph::{DagExecutor, Edge, Node, Pipeline};
use stygian_graph::ports::CachePort;
use proptest::prelude::*;

// ─── Graph invariants ─────────────────────────────────────────────────────────

proptest! {
    /// A chain of N unique nodes with N-1 sequential edges is always a valid DAG.
    #[test]
    fn prop_chain_pipeline_always_acyclic(n in 1usize..=20) {
        let mut p = Pipeline::new("chain");
        for i in 0..n {
            let node_id = format!("n{i}");
            p.add_node(Node::new(node_id, "noop", serde_json::json!({})));
        }
        for i in 1..n {
            let from = format!("n{}", i - 1);
            let to = format!("n{i}");
            p.add_edge(Edge::new(from, to));
        }
        // Chain is always acyclic → DagExecutor construction must succeed
        prop_assert!(DagExecutor::from_pipeline(&p).is_ok());
    }

    /// Any node with empty id must fail validation.
    #[test]
    fn prop_empty_node_id_always_invalid(service in "[a-z]{1,10}") {
        let node = Node::new("", service.as_str(), serde_json::json!({}));
        prop_assert!(node.validate().is_err());
    }

    /// Any node with empty service must fail validation.
    #[test]
    fn prop_empty_service_always_invalid(id in "[a-z]{1,10}") {
        let node = Node::new(id.as_str(), "", serde_json::json!({}));
        prop_assert!(node.validate().is_err());
    }

    /// A node with non-empty id and service always passes validation.
    #[test]
    fn prop_valid_node_always_validates(
        id in "[a-z][a-z0-9_-]{0,15}",
        service in "[a-z][a-z0-9_]{0,15}"
    ) {
        let node = Node::new(id.as_str(), service.as_str(), serde_json::json!({}));
        prop_assert!(node.validate().is_ok(), "valid node must pass: {node:?}");
    }

    /// An edge with an empty `from` field always fails validation.
    #[test]
    fn prop_empty_edge_from_always_invalid(to in "[a-z]{1,10}") {
        let edge = Edge::new("", to.as_str());
        prop_assert!(edge.validate().is_err());
    }

    /// An edge with an empty `to` field always fails validation.
    #[test]
    fn prop_empty_edge_to_always_invalid(from in "[a-z]{1,10}") {
        let edge = Edge::new(from.as_str(), "");
        prop_assert!(edge.validate().is_err());
    }

    /// A non-self-referencing edge with non-empty endpoints always validates.
    #[test]
    fn prop_valid_edge_validates(
        s in "[a-z][a-z0-9]{0,8}",
        t in "[A-Z][A-Z0-9]{0,8}",
    ) {
        // Use different character classes to guarantee s ≠ t
        let edge = Edge::new(s.as_str(), t.as_str());
        prop_assert!(edge.validate().is_ok(), "valid edge must pass: {edge:?}");
    }
}

// ─── Cache invariants ────────────────────────────────────────────────────────

proptest! {
    /// Whatever value is stored under a key, get returns that same value.
    #[test]
    fn prop_lru_cache_roundtrip(key in "[a-z]{1,20}", value in "[a-z]{1,100}") {
        let rt = tokio::runtime::Runtime::new().expect("tokio");
        rt.block_on(async {
            let cache = BoundedLruCache::new(NonZeroUsize::new(64).unwrap());
            cache.set(&key, value.clone(), None).await.expect("set");
            let got = cache.get(&key).await.expect("get");
            prop_assert_eq!(got, Some(value));
            Ok(())
        })?;
    }

    /// After invalidation, the key is always absent.
    #[test]
    fn prop_lru_after_invalidation_absent(key in "[a-z]{1,20}", value in "[a-z]{1,50}") {
        let rt = tokio::runtime::Runtime::new().expect("tokio");
        rt.block_on(async {
            let cache = BoundedLruCache::new(NonZeroUsize::new(64).unwrap());
            cache.set(&key, value, None).await.expect("set");
            cache.invalidate(&key).await.expect("invalidate");
            let gone = cache.get(&key).await.expect("get after invalidate");
            prop_assert!(gone.is_none());
            Ok(())
        })?;
    }
}

// ─── Metrics invariants ───────────────────────────────────────────────────────

proptest! {
    /// error_rate is always in [0.0, 1.0] when errors <= started.
    ///
    /// `error_rate` = `errors_total / requests_total`. `RequestCompleted { success: false }` is
    /// the error event while `RequestStarted` is the request event, so we constrain `errors <=
    /// started` to stay within a valid ratio.
    #[test]
    fn prop_error_rate_bounded(started in 1u64..=1000, error_frac in 0u64..=100) {
        let errors = (started * error_frac) / 100; // 0–100% of started
        let m = MetricsRegistry::new();
        for _ in 0..started {
            m.record(MetricEvent::RequestStarted { service: "s".into() });
        }
        for _ in 0..errors {
            m.record(MetricEvent::RequestCompleted {
                service: "s".into(),
                duration_ms: 0,
                success: false,
            });
        }
        let rate = m.snapshot().error_rate();
        prop_assert!((0.0..=1.0).contains(&rate), "error rate out of range: {rate}");
    }

    /// cache_hit_rate is always in [0.0, 1.0] regardless of hit/miss mix.
    #[test]
    fn prop_cache_hit_rate_bounded(hits in 0u64..=1000, misses in 0u64..=1000) {
        let m = MetricsRegistry::new();
        for _ in 0..hits {
            m.record(MetricEvent::CacheAccess { hit: true });
        }
        for _ in 0..misses {
            m.record(MetricEvent::CacheAccess { hit: false });
        }
        let rate = m.snapshot().cache_hit_rate();
        prop_assert!((0.0..=1.0).contains(&rate), "cache hit rate out of range: {rate}");
    }
}

// ─── RetryPolicy invariants ───────────────────────────────────────────────────

proptest! {
    /// delay_for never returns a duration exceeding max_delay, for any attempt index.
    #[test]
    fn prop_retry_delay_never_exceeds_max(
        max_ms in 100u64..=10_000,
        attempt in 0u32..=50,
    ) {
        let policy = RetryPolicy::new(
            100,
            Duration::from_millis(10),
            Duration::from_millis(max_ms),
        )
        .with_jitter_ms(0);

        let d = policy.delay_for(attempt);
        prop_assert!(
            d <= Duration::from_millis(max_ms),
            "delay {d:?} exceeds max {max_ms}ms at attempt {attempt}"
        );
    }

    /// delay_for always returns a non-negative (non-zero for attempt 0) duration.
    #[test]
    fn prop_retry_base_delay_non_negative(base_ms in 1u64..=1000, attempt in 0u32..=10) {
        let policy = RetryPolicy::new(
            20,
            Duration::from_millis(base_ms),
            Duration::from_millis(base_ms * 1024),
        )
        .with_jitter_ms(0);

        let d = policy.delay_for(attempt);
        prop_assert!(d >= Duration::from_millis(base_ms), "delay {d:?} below base {base_ms}ms");
    }
}
