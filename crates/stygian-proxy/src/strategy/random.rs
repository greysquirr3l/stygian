//! Random proxy rotation strategy.

use async_trait::async_trait;
use rand::prelude::IndexedRandom as _;

use crate::error::{ProxyError, ProxyResult};
use crate::strategy::{ProxyCandidate, RotationStrategy, healthy_candidates};

/// Selects a healthy proxy uniformly at random on each call.
///
/// Stateless — no lock or shared counter required.
///
/// # Example
/// ```
/// # tokio_test::block_on(async {
/// use stygian_proxy::strategy::{RandomStrategy, RotationStrategy, ProxyCandidate};
/// use stygian_proxy::types::ProxyMetrics;
/// use std::sync::Arc;
/// use uuid::Uuid;
///
/// let strategy = RandomStrategy;
/// let candidates = vec![
///     ProxyCandidate { id: Uuid::new_v4(), weight: 1, metrics: Arc::new(ProxyMetrics::default()), healthy: true, capabilities: Default::default() },
/// ];
/// strategy.select(&candidates).await.unwrap();
/// # })
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomStrategy;

#[async_trait]
impl RotationStrategy for RandomStrategy {
    async fn select<'a>(
        &self,
        candidates: &'a [ProxyCandidate],
    ) -> ProxyResult<&'a ProxyCandidate> {
        let healthy = healthy_candidates(candidates);
        healthy
            .choose(&mut rand::rng())
            .copied()
            .ok_or(ProxyError::AllProxiesUnhealthy)
    }
}
