//! Weighted random proxy rotation strategy.

use async_trait::async_trait;
use rand::RngExt as _;

use crate::error::{ProxyError, ProxyResult};
use crate::strategy::{ProxyCandidate, RotationStrategy, healthy_candidates};

/// Selects a healthy proxy with probability proportional to its `weight`.
///
/// Proxies with `weight == 0` are never selected.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::strategy::{WeightedStrategy, RotationStrategy, ProxyCandidate};
/// use stygian_proxy::types::ProxyMetrics;
/// use std::sync::Arc;
/// use uuid::Uuid;
///
/// let strategy = WeightedStrategy;
/// let candidates = vec![
///     ProxyCandidate { id: Uuid::new_v4(), weight: 10, metrics: Arc::new(ProxyMetrics::default()), healthy: true, capabilities: Default::default() },
///     ProxyCandidate { id: Uuid::new_v4(), weight: 1,  metrics: Arc::new(ProxyMetrics::default()), healthy: true, capabilities: Default::default() },
/// ];
/// strategy.select(&candidates).await.unwrap();
/// # })
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct WeightedStrategy;

#[async_trait]
impl RotationStrategy for WeightedStrategy {
    async fn select<'a>(
        &self,
        candidates: &'a [ProxyCandidate],
    ) -> ProxyResult<&'a ProxyCandidate> {
        let healthy: Vec<&ProxyCandidate> = healthy_candidates(candidates)
            .into_iter()
            .filter(|c| c.weight > 0)
            .collect();

        if healthy.is_empty() {
            return Err(ProxyError::AllProxiesUnhealthy);
        }

        let total: u64 = healthy.iter().map(|c| u64::from(c.weight)).sum();
        let mut cursor: u64 = rand::rng().random_range(0..total);

        for candidate in &healthy {
            if cursor < u64::from(candidate.weight) {
                return Ok(candidate);
            }
            cursor -= u64::from(candidate.weight);
        }

        // Unreachable: cursor always exhausts within the loop.
        healthy
            .last()
            .copied()
            .ok_or(crate::error::ProxyError::AllProxiesUnhealthy)
    }
}
