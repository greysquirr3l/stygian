//! Health check reporting for Kubernetes liveness and readiness probes.
//!
//! Provides structured health-check types and a [`HealthReporter`] for aggregating
//! component-level health into an overall [`HealthReport`].
//!
//! # Example
//!
//! ```
//! use mycelium_graph::application::health::{HealthReporter, HealthStatus, ComponentHealth};
//!
//! let mut reporter = HealthReporter::new();
//! reporter.register("database", HealthStatus::Healthy);
//! reporter.register("cache", HealthStatus::Degraded("High latency".to_string()));
//!
//! let report = reporter.report();
//! assert!(report.is_ready());  // Degraded is still operational ⇒ ready
//! assert!(report.is_live());   // Still alive while degraded
//! ```

use std::collections::HashMap;
use std::time::SystemTime;

use parking_lot::RwLock;

use serde::{Deserialize, Serialize};

// ─── HealthStatus ─────────────────────────────────────────────────────────────

/// The health status of a single component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "lowercase")]
pub enum HealthStatus {
    /// Component is operating normally.
    Healthy,
    /// Component is partially impaired but still serving requests.
    Degraded(String),
    /// Component is non-functional; requests will fail.
    Unhealthy(String),
}

impl HealthStatus {
    /// Returns `true` only when the component is fully healthy.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::HealthStatus;
    /// assert!(HealthStatus::Healthy.is_healthy());
    /// assert!(!HealthStatus::Degraded("latency".into()).is_healthy());
    /// ```
    pub const fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    /// Returns `true` when the component can still serve requests (healthy or degraded).
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::HealthStatus;
    /// assert!(HealthStatus::Healthy.is_operational());
    /// assert!(HealthStatus::Degraded("high latency".into()).is_operational());
    /// assert!(!HealthStatus::Unhealthy("connection refused".into()).is_operational());
    /// ```
    pub const fn is_operational(&self) -> bool {
        !matches!(self, Self::Unhealthy(_))
    }
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded(r) => write!(f, "degraded: {r}"),
            Self::Unhealthy(r) => write!(f, "unhealthy: {r}"),
        }
    }
}

// ─── ComponentHealth ─────────────────────────────────────────────────────────

/// Health state for a single named component.
///
/// Returned as part of a [`HealthReport`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component identifier (e.g. `"database"`, `"cache"`, `"worker_pool"`)
    pub name: String,
    /// Component status
    pub status: HealthStatus,
    /// Optional free-form details (timings, error messages, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ComponentHealth {
    /// Create a healthy component with no extra details.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{ComponentHealth, HealthStatus};
    ///
    /// let c = ComponentHealth::healthy("cache");
    /// assert_eq!(c.status, HealthStatus::Healthy);
    /// ```
    pub fn healthy(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            details: None,
        }
    }

    /// Create a degraded component.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::ComponentHealth;
    ///
    /// let c = ComponentHealth::degraded("database", "replication lag 5s");
    /// assert!(!c.status.is_healthy());
    /// ```
    pub fn degraded(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded(reason.into()),
            details: None,
        }
    }

    /// Create an unhealthy component.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::ComponentHealth;
    ///
    /// let c = ComponentHealth::unhealthy("valkey", "connection refused");
    /// assert!(!c.status.is_operational());
    /// ```
    pub fn unhealthy(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy(reason.into()),
            details: None,
        }
    }

    /// Attach arbitrary JSON details to this component.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::ComponentHealth;
    ///
    /// let c = ComponentHealth::healthy("http_pool")
    ///     .with_details(serde_json::json!({ "idle_connections": 8, "max": 32 }));
    ///
    /// assert!(c.details.is_some());
    /// ```
    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

// ─── HealthReport ────────────────────────────────────────────────────────────

/// Aggregated health report for all registered components.
///
/// Returned by [`HealthReporter::report`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Overall system status (worst of all components)
    pub overall: HealthStatus,
    /// Per-component breakdown
    pub components: Vec<ComponentHealth>,
    /// When this report was generated (Unix seconds)
    #[serde(with = "system_time_serde")]
    pub checked_at: SystemTime,
}

impl HealthReport {
    /// Returns `true` when the system is ready to serve traffic.
    ///
    /// The system is ready only when **all** components are healthy or degraded
    /// (Kubernetes readiness probe).
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, HealthStatus};
    ///
    /// let mut r = HealthReporter::new();
    /// r.register("db", HealthStatus::Healthy);
    /// assert!(r.report().is_ready());
    /// ```
    pub fn is_ready(&self) -> bool {
        self.components.iter().all(|c| c.status.is_operational())
    }

    /// Returns `true` while the process should continue running.
    ///
    /// The process is considered alive unless every component is unhealthy
    /// (Kubernetes liveness probe).
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, HealthStatus};
    ///
    /// let r = HealthReporter::new();
    /// r.register("db", HealthStatus::Unhealthy("disk full".into()));
    /// r.register("cache", HealthStatus::Healthy);
    /// // One unhealthy component doesn't kill the process while others are healthy
    /// assert!(r.report().is_live());
    /// ```
    pub fn is_live(&self) -> bool {
        // Dead when ALL components are unhealthy (or no components registered)
        if self.components.is_empty() {
            return true;
        }
        self.components.iter().any(|c| c.status.is_operational())
    }

    /// HTTP status code suitable for a health-check endpoint.
    ///
    /// Returns `200` when ready, `503` when not.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, HealthStatus};
    ///
    /// let mut r = HealthReporter::new();
    /// r.register("db", HealthStatus::Healthy);
    /// assert_eq!(r.report().http_status_code(), 200u16);
    /// ```
    pub fn http_status_code(&self) -> u16 {
        if self.is_ready() { 200 } else { 503 }
    }
}

// ─── System-time serde helper ─────────────────────────────────────────────────

mod system_time_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        s.serialize_u64(secs)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(UNIX_EPOCH + std::time::Duration::from_secs(secs))
    }
}

// ─── HealthReporter ──────────────────────────────────────────────────────────

/// Collects component-level health checks and produces a [`HealthReport`].
///
/// Thread-safe; cheaply cloneable via `Arc` patterns.
///
/// # Example
///
/// ```
/// use mycelium_graph::application::health::{HealthReporter, HealthStatus, ComponentHealth};
///
/// let mut reporter = HealthReporter::new();
/// reporter.register("database", HealthStatus::Healthy);
/// reporter.register_component(
///     ComponentHealth::degraded("cache", "latency p99 > 100ms")
///         .with_details(serde_json::json!({ "p99_ms": 142 }))
/// );
///
/// let report = reporter.report();
/// assert_eq!(report.http_status_code(), 200u16); // degraded is operational
/// ```
pub struct HealthReporter {
    components: RwLock<HashMap<String, ComponentHealth>>,
}

impl HealthReporter {
    /// Create an empty reporter.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::HealthReporter;
    ///
    /// let r = HealthReporter::new();
    /// assert!(r.report().components.is_empty());
    /// ```
    pub fn new() -> Self {
        Self {
            components: RwLock::new(HashMap::new()),
        }
    }

    /// Register or update a component's status by name.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, HealthStatus};
    ///
    /// let mut r = HealthReporter::new();
    /// r.register("db", HealthStatus::Healthy);
    /// assert_eq!(r.report().components.len(), 1);
    /// ```
    pub fn register(&self, name: impl Into<String>, status: HealthStatus) {
        let name = name.into();
        let component = ComponentHealth {
            name: name.clone(),
            status,
            details: None,
        };
        self.components.write().insert(name, component);
    }

    /// Register or update a component with full [`ComponentHealth`].
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, ComponentHealth};
    ///
    /// let mut r = HealthReporter::new();
    /// r.register_component(ComponentHealth::healthy("cache"));
    /// assert_eq!(r.report().components.len(), 1);
    /// ```
    pub fn register_component(&self, component: ComponentHealth) {
        self.components
            .write()
            .insert(component.name.clone(), component);
    }

    /// Remove a component from reporting.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, HealthStatus};
    ///
    /// let mut r = HealthReporter::new();
    /// r.register("db", HealthStatus::Healthy);
    /// r.deregister("db");
    /// assert!(r.report().components.is_empty());
    /// ```
    pub fn deregister(&self, name: &str) {
        self.components.write().remove(name);
    }

    /// Generate a [`HealthReport`] from current component states.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::health::{HealthReporter, HealthStatus};
    ///
    /// let r = HealthReporter::new();
    /// let report = r.report();
    /// assert_eq!(report.overall, HealthStatus::Healthy);
    /// assert!(report.is_live());
    /// ```
    pub fn report(&self) -> HealthReport {
        let components: Vec<ComponentHealth> = self.components.read().values().cloned().collect();

        let overall = aggregate_status(&components);
        HealthReport {
            overall,
            components,
            checked_at: SystemTime::now(),
        }
    }
}

impl Default for HealthReporter {
    fn default() -> Self {
        Self::new()
    }
}

fn aggregate_status(components: &[ComponentHealth]) -> HealthStatus {
    let mut worst = HealthStatus::Healthy;
    for c in components {
        match &c.status {
            HealthStatus::Unhealthy(r) => {
                return HealthStatus::Unhealthy(r.clone());
            }
            HealthStatus::Degraded(r) => {
                if worst == HealthStatus::Healthy {
                    worst = HealthStatus::Degraded(r.clone());
                }
            }
            HealthStatus::Healthy => {}
        }
    }
    worst
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn healthy_status_is_healthy() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(HealthStatus::Healthy.is_operational());
    }

    #[test]
    fn degraded_status_is_not_healthy_but_operational() {
        let s = HealthStatus::Degraded("reason".into());
        assert!(!s.is_healthy());
        assert!(s.is_operational());
    }

    #[test]
    fn unhealthy_status_is_not_operational() {
        let s = HealthStatus::Unhealthy("crashed".into());
        assert!(!s.is_healthy());
        assert!(!s.is_operational());
    }

    #[test]
    fn empty_reporter_overall_is_healthy() {
        let reporter = HealthReporter::new();
        assert_eq!(reporter.report().overall, HealthStatus::Healthy);
    }

    #[test]
    fn all_healthy_report_is_ready_and_live() {
        let r = HealthReporter::new();
        r.register("db", HealthStatus::Healthy);
        r.register("cache", HealthStatus::Healthy);
        let report = r.report();
        assert!(report.is_ready());
        assert!(report.is_live());
        assert_eq!(report.http_status_code(), 200);
    }

    #[test]
    fn degraded_component_report_not_ready_but_still_live() {
        let r = HealthReporter::new();
        r.register("db", HealthStatus::Healthy);
        r.register("cache", HealthStatus::Degraded("high latency".into()));
        let report = r.report();
        // Degraded is operational so is_ready returns true
        assert!(report.is_ready());
        assert!(report.is_live());
    }

    #[test]
    fn unhealthy_component_makes_report_not_ready() {
        let r = HealthReporter::new();
        r.register("db", HealthStatus::Unhealthy("connection refused".into()));
        let report = r.report();
        assert!(!report.is_ready());
        assert_eq!(report.http_status_code(), 503);
    }

    #[test]
    fn all_unhealthy_not_live() {
        let r = HealthReporter::new();
        r.register("a", HealthStatus::Unhealthy("x".into()));
        r.register("b", HealthStatus::Unhealthy("y".into()));
        assert!(!r.report().is_live());
    }

    #[test]
    fn register_same_component_updates_status() {
        let r = HealthReporter::new();
        r.register("db", HealthStatus::Healthy);
        r.register("db", HealthStatus::Unhealthy("disk full".into()));
        let report = r.report();
        assert_eq!(report.components.len(), 1);
        assert!(!report.is_ready());
    }

    #[test]
    fn deregister_removes_component() {
        let r = HealthReporter::new();
        r.register("db", HealthStatus::Healthy);
        r.deregister("db");
        assert!(r.report().components.is_empty());
    }

    #[test]
    fn component_health_builders() {
        assert!(ComponentHealth::healthy("x").status.is_healthy());
        assert!(
            ComponentHealth::degraded("x", "slow")
                .status
                .is_operational()
        );
        assert!(
            !ComponentHealth::unhealthy("x", "down")
                .status
                .is_operational()
        );
    }

    #[test]
    fn component_with_details_serializes() {
        let c = ComponentHealth::healthy("pool").with_details(serde_json::json!({ "idle": 8 }));
        assert!(c.details.is_some());
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("idle"));
    }

    #[test]
    fn health_report_serializes_to_json() {
        let r = HealthReporter::new();
        r.register("db", HealthStatus::Healthy);
        let report = r.report();
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(json.contains("healthy"));
    }

    #[test]
    fn aggregate_status_worst_wins() {
        let components = vec![
            ComponentHealth::healthy("a"),
            ComponentHealth::degraded("b", "slow"),
            ComponentHealth::unhealthy("c", "down"),
        ];
        let status = aggregate_status(&components);
        assert!(matches!(status, HealthStatus::Unhealthy(_)));
    }
}
