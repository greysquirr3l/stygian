//! Proxy rotation strategy trait and built-in implementations.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::ProxyResult;
use crate::types::{CapabilityRequirement, ProxyCapabilities, ProxyMetrics};

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
    /// Protocol-level capabilities this proxy exposes.
    pub capabilities: ProxyCapabilities,
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
    /// Returns [`crate::error::ProxyError::AllProxiesUnhealthy`] when every candidate has
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

/// Filter `all` to candidates that are healthy **and** satisfy `req`.
///
/// An empty [`CapabilityRequirement`] (all flags `false`, no geo filter)
/// behaves identically to [`healthy_candidates`].
///
/// # Example
/// ```
/// use std::sync::Arc;
/// use stygian_proxy::strategy::{ProxyCandidate, capable_healthy_candidates};
/// use stygian_proxy::types::{CapabilityRequirement, ProxyCapabilities, ProxyMetrics};
/// use uuid::Uuid;
///
/// let caps = ProxyCapabilities { supports_https_connect: true, ..Default::default() };
/// let candidate = ProxyCandidate {
///     id: Uuid::new_v4(),
///     weight: 1,
///     metrics: Arc::new(ProxyMetrics::default()),
///     healthy: true,
///     capabilities: caps,
/// };
/// let req = CapabilityRequirement { require_https_connect: true, ..Default::default() };
/// let result = capable_healthy_candidates(&[candidate], &req);
/// assert_eq!(result.len(), 1);
/// ```
pub fn capable_healthy_candidates<'a>(
    all: &'a [ProxyCandidate],
    req: &CapabilityRequirement,
) -> Vec<&'a ProxyCandidate> {
    all.iter()
        .filter(|c| c.healthy && c.capabilities.satisfies(req))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::error::ProxyError;
    use crate::types::{CapabilityRequirement, ProxyCapabilities};
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
            capabilities: ProxyCapabilities::default(),
        }
    }

    /// Build a `ProxyCandidate` with explicit capabilities.
    pub fn candidate_with_caps(
        id: u128,
        healthy: bool,
        weight: u32,
        caps: ProxyCapabilities,
    ) -> ProxyCandidate {
        let metrics = Arc::new(ProxyMetrics::default());
        ProxyCandidate {
            id: Uuid::from_u128(id),
            weight,
            metrics,
            healthy,
            capabilities: caps,
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
        assert!(matches!(
            RoundRobinStrategy::default().select(&c).await,
            Err(ProxyError::AllProxiesUnhealthy)
        ));
    }

    #[test]
    fn capable_healthy_candidates_filters_by_capability() {
        let c = vec![
            candidate_with_caps(
                1,
                true,
                1,
                ProxyCapabilities {
                    supports_https_connect: true,
                    ..Default::default()
                },
            ),
            candidate_with_caps(2, true, 1, ProxyCapabilities::default()),
            candidate_with_caps(
                3,
                false,
                1,
                ProxyCapabilities {
                    supports_https_connect: true,
                    ..Default::default()
                },
            ),
        ];
        let req = CapabilityRequirement {
            require_https_connect: true,
            ..Default::default()
        };
        let result = capable_healthy_candidates(&c, &req);
        // Only candidate 1: healthy AND supports_https_connect
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.first().map(|candidate| candidate.id),
            Some(Uuid::from_u128(1))
        );
    }

    #[test]
    fn capable_healthy_candidates_empty_req_behaves_like_healthy() {
        let c = vec![
            candidate(1, true, 1, 0),
            candidate(2, false, 1, 0),
            candidate(3, true, 1, 0),
        ];
        let req = CapabilityRequirement::default();
        let result = capable_healthy_candidates(&c, &req);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn capable_healthy_candidates_returns_empty_when_none_match() {
        let c = vec![candidate(1, true, 1, 0), candidate(2, true, 1, 0)];
        let req = CapabilityRequirement {
            require_socks5_udp: true,
            ..Default::default()
        };
        let result = capable_healthy_candidates(&c, &req);
        assert!(result.is_empty());
    }

    #[test]
    fn geo_country_filter_matches_exact_country() {
        let gb_proxy_caps = ProxyCapabilities {
            geo_country: Some("GB".into()),
            ..Default::default()
        };
        let us_proxy_caps = ProxyCapabilities {
            geo_country: Some("US".into()),
            ..Default::default()
        };
        let c = vec![
            candidate_with_caps(1, true, 1, gb_proxy_caps),
            candidate_with_caps(2, true, 1, us_proxy_caps),
            candidate_with_caps(3, true, 1, ProxyCapabilities::default()),
        ];
        let req = CapabilityRequirement {
            require_geo_country: Some("GB".into()),
            ..Default::default()
        };
        let result = capable_healthy_candidates(&c, &req);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.first().map(|candidate| candidate.id),
            Some(Uuid::from_u128(1))
        );
    }
}
