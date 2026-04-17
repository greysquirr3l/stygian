//! Least-used proxy rotation strategy.

use std::sync::atomic::Ordering;

use async_trait::async_trait;

use crate::error::{ProxyError, ProxyResult};
use crate::strategy::{ProxyCandidate, RotationStrategy, healthy_candidates};

/// Selects the healthy proxy with the fewest total requests.
///
/// Ties are broken by position (the first minimum in the slice wins),
/// giving stable and predictable behaviour.
///
/// Runs in O(n) over the healthy candidate slice; suitable for pools of up to
/// ~10,000 proxies.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::strategy::{LeastUsedStrategy, RotationStrategy, ProxyCandidate};
/// use stygian_proxy::types::ProxyMetrics;
/// use std::sync::{Arc, atomic::Ordering};
/// use uuid::Uuid;
///
/// let strategy = LeastUsedStrategy;
/// let busy = Arc::new(ProxyMetrics::default());
/// busy.requests_total.store(100, Ordering::Relaxed);
/// let idle = Arc::new(ProxyMetrics::default());
/// let candidates = vec![
///     ProxyCandidate { id: Uuid::from_u128(1), weight: 1, metrics: busy,   healthy: true, capabilities: Default::default() },
///     ProxyCandidate { id: Uuid::from_u128(2), weight: 1, metrics: idle,   healthy: true, capabilities: Default::default() },
/// ];
/// let chosen = strategy.select(&candidates).await.unwrap();
/// assert_eq!(chosen.id, Uuid::from_u128(2), "should pick the idle proxy");
/// # })
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct LeastUsedStrategy;

#[async_trait]
impl RotationStrategy for LeastUsedStrategy {
    async fn select<'a>(
        &self,
        candidates: &'a [ProxyCandidate],
    ) -> ProxyResult<&'a ProxyCandidate> {
        let healthy = healthy_candidates(candidates);
        healthy
            .into_iter()
            .min_by_key(|c| c.metrics.requests_total.load(Ordering::Relaxed))
            .ok_or(ProxyError::AllProxiesUnhealthy)
    }
}
