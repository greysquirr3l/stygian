//! Fallback chain service adapter.
//!
//! Implements [`ScrapingService`] by trying a prioritised list of inner services
//! with per-service circuit breakers.  When a service's circuit is **Open** or
//! its execution fails, the chain automatically moves to the next lower-priority
//! service.
//!
//! # Behaviour
//!
//! 1. Services are tried in registration order (index 0 = highest priority).
//! 2. A service is **skipped** when its circuit breaker is [`CircuitState::Open`]
//!    and the reset timeout has not yet elapsed.  The chain then probes it once
//!    the timeout passes (half-open probe).
//! 3. On **success** the corresponding circuit breaker records the success and the
//!    result is returned immediately — no further services are tried.
//! 4. On **failure** the circuit breaker records the failure and the next service
//!    is tried.
//! 5. If every service is exhausted the last error is propagated.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use std::time::Duration;
//! use stygian_graph::adapters::fallback::FallbackChainService;
//! use stygian_graph::adapters::noop::NoopService;
//! use stygian_graph::adapters::resilience::CircuitBreakerImpl;
//!
//! let chain = FallbackChainService::builder()
//!     .add(Arc::new(NoopService), CircuitBreakerImpl::new(3, Duration::from_secs(30)))
//!     .named("primary-with-plugin-fallback")
//!     .build();
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::adapters::resilience::CircuitBreakerImpl;
use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::{CircuitBreaker, CircuitState, ScrapingService, ServiceInput, ServiceOutput};

// ── Chain entry ───────────────────────────────────────────────────────────────

/// A single link in the fallback chain: a service paired with its circuit breaker.
struct ChainEntry {
    service: Arc<dyn ScrapingService>,
    breaker: Arc<CircuitBreakerImpl>,
}

// ── FallbackChainService ──────────────────────────────────────────────────────

/// A [`ScrapingService`] that tries multiple inner services in priority order,
/// automatically routing around open circuit breakers and failed services.
///
/// Construct via [`FallbackChainService::builder()`].
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use stygian_graph::adapters::fallback::FallbackChainService;
/// use stygian_graph::adapters::noop::NoopService;
/// use stygian_graph::adapters::resilience::CircuitBreakerImpl;
/// use stygian_graph::ports::ScrapingService;
///
/// let chain = FallbackChainService::builder()
///     .add(Arc::new(NoopService), CircuitBreakerImpl::new(5, Duration::from_secs(60)))
///     .build();
///
/// assert_eq!(chain.name(), "fallback-chain");
/// ```
pub struct FallbackChainService {
    entries: Vec<ChainEntry>,
    name: &'static str,
}

impl FallbackChainService {
    /// Return a [`FallbackChainBuilder`] for ergonomic construction.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::fallback::FallbackChainService;
    ///
    /// let builder = FallbackChainService::builder();
    /// ```
    pub const fn builder() -> FallbackChainBuilder {
        FallbackChainBuilder::new()
    }

    /// Return the number of services in this chain.
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use stygian_graph::adapters::fallback::FallbackChainService;
    /// use stygian_graph::adapters::noop::NoopService;
    /// use stygian_graph::adapters::resilience::CircuitBreakerImpl;
    ///
    /// let chain = FallbackChainService::builder()
    ///     .add(Arc::new(NoopService), CircuitBreakerImpl::new(3, Duration::from_secs(30)))
    ///     .add(Arc::new(NoopService), CircuitBreakerImpl::new(3, Duration::from_secs(30)))
    ///     .build();
    ///
    /// assert_eq!(chain.len(), 2);
    /// ```
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` when no services are registered.
    ///
    /// An empty chain always returns [`ServiceError::Unavailable`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::fallback::FallbackChainService;
    ///
    /// let chain = FallbackChainService::builder().build();
    /// assert!(chain.is_empty());
    /// ```
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[async_trait]
impl ScrapingService for FallbackChainService {
    /// Execute the fallback chain.
    ///
    /// Tries each registered service in order, respecting circuit breaker state.
    /// Returns the first successful result, or the last error if all services fail.
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let mut last_err: Option<StygianError> = None;

        for (idx, entry) in self.entries.iter().enumerate() {
            let state = entry.breaker.state();

            // Skip services whose circuit is Open and hasn't timed out yet.
            if state == CircuitState::Open {
                if !entry.breaker.attempt_reset() {
                    debug!(
                        service = entry.service.name(),
                        chain = self.name,
                        idx,
                        "circuit open — skipping service in fallback chain"
                    );
                    continue;
                }
                debug!(
                    service = entry.service.name(),
                    chain = self.name,
                    idx,
                    "circuit half-open — probing service"
                );
            }

            debug!(
                service = entry.service.name(),
                chain = self.name,
                idx,
                url = %input.url,
                "fallback chain: attempting service"
            );

            match entry.service.execute(input.clone()).await {
                Ok(output) => {
                    entry.breaker.record_success();
                    info!(
                        service = entry.service.name(),
                        chain = self.name,
                        idx,
                        "fallback chain: service succeeded"
                    );
                    return Ok(output);
                }
                Err(e) => {
                    entry.breaker.record_failure();
                    warn!(
                        service = entry.service.name(),
                        chain = self.name,
                        idx,
                        error = %e,
                        "fallback chain: service failed — advancing to next"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "fallback chain '{}' exhausted: no services registered or available",
                self.name
            )))
        }))
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

// ── FallbackChainBuilder ──────────────────────────────────────────────────────

/// Builder for [`FallbackChainService`].
///
/// Services are tried in the order they are added.  Add the highest-priority
/// (cheapest / most reliable) service first; add the plugin extraction adapter
/// last as the final fallback.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use stygian_graph::adapters::fallback::FallbackChainBuilder;
/// use stygian_graph::adapters::noop::NoopService;
/// use stygian_graph::adapters::resilience::CircuitBreakerImpl;
/// use stygian_graph::ports::ScrapingService;
///
/// let chain = FallbackChainBuilder::new()
///     .add(Arc::new(NoopService), CircuitBreakerImpl::new(5, Duration::from_secs(60)))
///     .add(Arc::new(NoopService), CircuitBreakerImpl::new(3, Duration::from_secs(30)))
///     .named("http-to-plugin")
///     .build();
///
/// assert_eq!(chain.len(), 2);
/// assert_eq!(chain.name(), "http-to-plugin");
/// ```
pub struct FallbackChainBuilder {
    entries: Vec<ChainEntry>,
    name: &'static str,
}

impl FallbackChainBuilder {
    /// Create an empty builder with the default name `"fallback-chain"`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::fallback::FallbackChainBuilder;
    ///
    /// let builder = FallbackChainBuilder::new();
    /// ```
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            name: "fallback-chain",
        }
    }

    /// Add a service and its dedicated circuit breaker (highest to lowest priority).
    ///
    /// # Arguments
    ///
    /// * `service` — The [`ScrapingService`] to add.
    /// * `breaker` — A [`CircuitBreakerImpl`] configured for this specific service.
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use stygian_graph::adapters::fallback::FallbackChainBuilder;
    /// use stygian_graph::adapters::noop::NoopService;
    /// use stygian_graph::adapters::resilience::CircuitBreakerImpl;
    ///
    /// let builder = FallbackChainBuilder::new()
    ///     .add(Arc::new(NoopService), CircuitBreakerImpl::new(5, Duration::from_secs(60)));
    /// ```
    #[must_use]
    pub fn add(mut self, service: Arc<dyn ScrapingService>, breaker: CircuitBreakerImpl) -> Self {
        self.entries.push(ChainEntry {
            service,
            breaker: Arc::new(breaker),
        });
        self
    }

    /// Override the static name reported by [`ScrapingService::name`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::fallback::FallbackChainBuilder;
    /// use stygian_graph::ports::ScrapingService;
    ///
    /// let chain = FallbackChainBuilder::new().named("http-to-plugin-fallback").build();
    /// assert_eq!(chain.name(), "http-to-plugin-fallback");
    /// ```
    #[must_use]
    pub const fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Build the [`FallbackChainService`].
    ///
    /// An empty chain (no services added) is valid but will immediately return
    /// [`ServiceError::Unavailable`] on every call.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::fallback::FallbackChainBuilder;
    ///
    /// let chain = FallbackChainBuilder::new().build();
    /// assert!(chain.is_empty());
    /// ```
    pub fn build(self) -> FallbackChainService {
        FallbackChainService {
            entries: self.entries,
            name: self.name,
        }
    }
}

impl Default for FallbackChainBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Default circuit breaker parameters ───────────────────────────────────────

/// Sensible default for a production circuit breaker on a primary scraper.
///
/// Opens after **5 consecutive failures** and attempts reset after **30 seconds**.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::fallback::default_primary_breaker;
///
/// let breaker = default_primary_breaker();
/// ```
pub fn default_primary_breaker() -> CircuitBreakerImpl {
    CircuitBreakerImpl::new(5, Duration::from_secs(30))
}

/// Sensible default for a production circuit breaker on a fallback scraper.
///
/// Opens after **3 consecutive failures** and attempts reset after **60 seconds**.
/// The longer reset timeout gives the fallback more time to recover since it is
/// typically a heavier operation.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::fallback::default_fallback_breaker;
///
/// let breaker = default_fallback_breaker();
/// ```
pub fn default_fallback_breaker() -> CircuitBreakerImpl {
    #[allow(clippy::duration_suboptimal_units)]
    {
        CircuitBreakerImpl::new(3, Duration::from_secs(60))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::noop::NoopService;
    use crate::domain::error::ServiceError;
    use crate::ports::{ServiceInput, ServiceOutput};
    use serde_json::json;

    // ── helper: always-failing service ────────────────────────────────────

    struct AlwaysFailService;

    #[async_trait]
    impl ScrapingService for AlwaysFailService {
        async fn execute(&self, _input: ServiceInput) -> Result<ServiceOutput> {
            Err(StygianError::Service(ServiceError::Unavailable(
                "simulated failure".into(),
            )))
        }

        fn name(&self) -> &'static str {
            "always-fail"
        }
    }

    fn make_input() -> ServiceInput {
        ServiceInput {
            url: "https://example.com".to_string(),
            params: json!({}),
        }
    }

    // ── tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_first_service_succeeds() -> Result<()> {
        let chain = FallbackChainService::builder()
            .add(
                Arc::new(NoopService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .add(
                Arc::new(AlwaysFailService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .build();

        let output = chain.execute(make_input()).await?;
        match output.metadata.get("service") {
            Some(service) => assert_eq!(service, "noop", "noop should win"),
            None => {
                return Err(
                    ServiceError::Unavailable("service key should exist".to_string()).into(),
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_fallback_fires_when_primary_fails() -> Result<()> {
        let chain = FallbackChainService::builder()
            .add(
                Arc::new(AlwaysFailService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .add(
                Arc::new(NoopService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .named("primary-then-noop")
            .build();

        let output = chain.execute(make_input()).await?;
        match output.metadata.get("service") {
            Some(service) => assert_eq!(
                service, "noop",
                "fallback noop should win after primary failure"
            ),
            None => {
                return Err(
                    ServiceError::Unavailable("service key should exist".to_string()).into(),
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_all_services_fail_returns_error() {
        let chain = FallbackChainService::builder()
            .add(
                Arc::new(AlwaysFailService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .add(
                Arc::new(AlwaysFailService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .build();

        let result = chain.execute(make_input()).await;
        assert!(result.is_err(), "all-failing chain must return error");
    }

    #[tokio::test]
    async fn test_empty_chain_returns_unavailable() {
        let chain = FallbackChainService::builder().build();
        let result = chain.execute(make_input()).await;
        assert!(
            result.is_err(),
            "empty chain must return ServiceError::Unavailable"
        );
    }

    #[tokio::test]
    async fn test_chain_name_default() {
        let chain = FallbackChainService::builder().build();
        assert_eq!(chain.name(), "fallback-chain");
    }

    #[tokio::test]
    async fn test_chain_name_custom() {
        let chain = FallbackChainService::builder()
            .named("http-to-plugin")
            .build();
        assert_eq!(chain.name(), "http-to-plugin");
    }

    #[tokio::test]
    async fn test_open_circuit_skipped_advances_to_next() -> Result<()> {
        // Breaker with threshold 1: one failure opens the circuit
        let failing_breaker = CircuitBreakerImpl::new(1, {
            #[allow(clippy::duration_suboptimal_units)]
            {
                Duration::from_secs(3600)
            } // 1 hour
        });

        // Pre-open the circuit by recording the one required failure
        failing_breaker.record_failure();
        assert_eq!(
            failing_breaker.state(),
            CircuitState::Open,
            "breaker should be open after threshold hit"
        );

        let chain = FallbackChainService::builder()
            .add(Arc::new(AlwaysFailService), failing_breaker)
            .add(
                Arc::new(NoopService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .named("open-circuit-skip-test")
            .build();

        // The first service's circuit is open and the timeout is 3600s so it
        // should be skipped entirely, and noop (second) should succeed.
        let output = chain.execute(make_input()).await?;
        match output.metadata.get("service") {
            Some(service) => assert_eq!(
                service, "noop",
                "open-circuit service must be skipped; noop must serve the request"
            ),
            None => {
                return Err(
                    ServiceError::Unavailable("service key should exist".to_string()).into(),
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_circuit_records_success_on_recovery() -> Result<()> {
        // Build a chain with two noop services
        let chain = FallbackChainService::builder()
            .add(
                Arc::new(NoopService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .build();

        // Execute twice — both should succeed and circuit stays closed
        chain.execute(make_input()).await?;
        chain.execute(make_input()).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let empty = FallbackChainService::builder().build();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let one = FallbackChainService::builder()
            .add(
                Arc::new(NoopService),
                CircuitBreakerImpl::new(5, Duration::from_secs(30)),
            )
            .build();
        assert!(!one.is_empty());
        assert_eq!(one.len(), 1);
    }

    #[tokio::test]
    async fn test_default_breaker_helpers() {
        let primary = default_primary_breaker();
        let fallback = default_fallback_breaker();
        assert_eq!(primary.state(), CircuitState::Closed);
        assert_eq!(fallback.state(), CircuitState::Closed);
    }
}
