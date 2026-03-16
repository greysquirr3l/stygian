//! Proxy rotation strategy trait and built-in implementations.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::ProxyResult;
use crate::types::ProxyMetrics;

mod least_used;
mod random;
mod round_robin;
mod weighted;

pub use least_used::LeastUsedStrategy;
pub use random::RandomStrategy;
pub use round_robin::RoundRobinStrategy;
pub use weighted::WeightedStrategy;

// ─────────────────────────────────────────────────────────────────────────────
// ProxyCandidate
// ─────────────────────────────────────────────────────────────────────────────

/// A lightweight view of a proxy considered for selection.
///
/// Strategies operate on slices of `ProxyCandidate` values built from the live
/// proxy pool. The `metrics` field allows latency- or usage-aware selection
/// without acquiring a write lock.
#[derive(Debug, Clone)]
pub struct ProxyCandidate {
    /// Stable identifier matching the [`ProxyRecord`](crate::types::ProxyRecord).
    pub id: Uuid,
    /// Relative weight used by [`WeightedStrategy`].
    pub weight: u32,
    /// Shared atomics updated by every request through this proxy.
    pub metrics: Arc<ProxyMetrics>,
    /// Whether the proxy currently passes health checks.
    pub healthy: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// RotationStrategy trait
// ─────────────────────────────────────────────────────────────────────────────

/// Selects a proxy from a slice of candidates on each request.
///
/// Implementations receive **all** candidates (healthy and unhealthy) so they
/// can distinguish between an empty pool and a pool where every proxy is
/// temporarily down. Call [`healthy_candidates`] to filter the slice.
///
/// # Example
/// ```rust,no_run
/// use stygian_proxy::strategy::{ProxyCandidate, RotationStrategy, RoundRobinStrategy};
///
/// async fn pick(candidates: &[ProxyCandidate]) {
///     let strategy = RoundRobinStrategy::default();
///     let chosen = strategy.select(candidates).await.unwrap();
///     println!("selected: {}", chosen.id);
/// }
/// ```
#[async_trait]
pub trait RotationStrategy: Send + Sync + 'static {
    /// Select one candidate from `candidates`.
    ///
    /// Returns [`ProxyError::AllProxiesUnhealthy`] when every candidate has
    /// `healthy == false`.
    async fn select<'a>(&self, candidates: &'a [ProxyCandidate])
    -> ProxyResult<&'a ProxyCandidate>;
}

/// Shared-ownership type alias for a [`RotationStrategy`] implementation.
pub type BoxedRotationStrategy = Arc<dyn RotationStrategy>;

// ─────────────────────────────────────────────────────────────────────────────
// Shared helper
// ─────────────────────────────────────────────────────────────────────────────

/// Filter `all` to only the candidates that are currently healthy.
///
/// Returns references into the original slice, so no allocation is needed
/// beyond the returned `Vec`.
pub fn healthy_candidates(all: &[ProxyCandidate]) -> Vec<&ProxyCandidate> {
    all.iter().filter(|c| c.healthy).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::error::ProxyError;
    use std::sync::atomic::Ordering;

    /// Build a `ProxyCandidate` with sensible test defaults.
    pub fn candidate(id: u128, healthy: bool, weight: u32, requests: u64) -> ProxyCandidate {
        let metrics = Arc::new(ProxyMetrics::default());
        metrics.requests_total.store(requests, Ordering::Relaxed);
        ProxyCandidate {
            id: Uuid::from_u128(id),
            weight,
            metrics,
            healthy,
        }
    }

    #[tokio::test]
    async fn healthy_candidates_filters() {
        let c = vec![
            candidate(1, true, 1, 0),
            candidate(2, false, 1, 0),
            candidate(3, true, 1, 0),
        ];
        let healthy = healthy_candidates(&c);
        assert_eq!(healthy.len(), 2);
        assert!(healthy.iter().all(|c| c.healthy));
    }

    #[tokio::test]
    async fn all_unhealthy_returns_error() {
        let c = vec![candidate(1, false, 1, 0), candidate(2, false, 1, 0)];
        let err = RoundRobinStrategy::default().select(&c).await.unwrap_err();
        assert!(matches!(err, ProxyError::AllProxiesUnhealthy));
    }
}
