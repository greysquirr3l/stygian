//! Round-robin proxy rotation strategy.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::error::ProxyResult;
use crate::strategy::{ProxyCandidate, RotationStrategy, healthy_candidates};

/// Cycles through healthy proxies in order, distributing load evenly.
///
/// Uses a lock-free [`AtomicUsize`] counter, so no `Mutex` is needed even
/// under high concurrency.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::strategy::{RoundRobinStrategy, RotationStrategy, ProxyCandidate};
/// use stygian_proxy::types::ProxyMetrics;
/// use std::sync::Arc;
/// use uuid::Uuid;
///
/// let strategy = RoundRobinStrategy::default();
/// let candidates = vec![
///     ProxyCandidate { id: Uuid::new_v4(), weight: 1, metrics: Arc::new(ProxyMetrics::default()), healthy: true },
///     ProxyCandidate { id: Uuid::new_v4(), weight: 1, metrics: Arc::new(ProxyMetrics::default()), healthy: true },
/// ];
/// let a = strategy.select(&candidates).await.unwrap().id;
/// let b = strategy.select(&candidates).await.unwrap().id;
/// assert_ne!(a, b, "round-robin should alternate between two proxies");
/// # })
/// ```
#[derive(Debug, Default)]
pub struct RoundRobinStrategy {
    counter: AtomicUsize,
}

#[async_trait]
impl RotationStrategy for RoundRobinStrategy {
    async fn select<'a>(
        &self,
        candidates: &'a [ProxyCandidate],
    ) -> ProxyResult<&'a ProxyCandidate> {
        use crate::error::ProxyError;

        let healthy = healthy_candidates(candidates);
        if healthy.is_empty() {
            return Err(ProxyError::AllProxiesUnhealthy);
        }
        let idx = self
            .counter
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_rem(healthy.len());
        healthy
            .get(idx)
            .copied()
            .ok_or(ProxyError::AllProxiesUnhealthy)
    }
}
