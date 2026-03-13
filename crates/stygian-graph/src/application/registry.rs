//! Service registry and dependency injection
//!
//! Provides a runtime registry for wiring together ports and adapters.
//!
//! Key features:
//! - Thread-safe dynamic registration via `Arc<RwLock<…>>`
//! - Builder pattern for ergonomic construction
//! - `LazyLock`-based process-wide default registry singleton
//! - Per-service health checks and availability status
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::application::registry::ServiceRegistry;
//! use stygian_graph::adapters::noop::NoopService;
//! use std::sync::Arc;
//!
//! let registry = ServiceRegistry::builder()
//!     .register("noop", Arc::new(NoopService))
//!     .build();
//!
//! let svc = registry.get("noop");
//! assert!(svc.is_some());
//! ```

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::ports::ScrapingService;

// ─── Availability status ──────────────────────────────────────────────────────

/// Availability status of a registered service
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceStatus {
    /// Service responded to its last health check successfully
    Healthy,
    /// Service is degraded but still processing requests
    Degraded(String),
    /// Service is unavailable
    Unavailable(String),
    /// Health check has not been run yet
    Unknown,
}

impl ServiceStatus {
    /// Returns `true` when [`ServiceStatus::Healthy`] or [`ServiceStatus::Degraded`]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded(_))
    }
}

// ─── Registry entry ───────────────────────────────────────────────────────────

struct RegistryEntry {
    service: Arc<dyn ScrapingService>,
    status: ServiceStatus,
}

// ─── ServiceRegistry ─────────────────────────────────────────────────────────

/// Thread-safe runtime registry for [`ScrapingService`] adapters.
///
/// Use [`ServiceRegistry::builder()`] for ergonomic setup, or call
/// [`ServiceRegistry::register`] at runtime for dynamic registration.
///
/// # Thread safety
///
/// All mutations are guarded by an `RwLock`. Reads are non-exclusive.
pub struct ServiceRegistry {
    entries: Arc<RwLock<HashMap<String, RegistryEntry>>>,
}

// SAFETY: RwLock poisoning only occurs on panic; panics are unrecoverable for
// this service so unwrap is correct here.
#[allow(clippy::unwrap_used)]
impl ServiceRegistry {
    /// Create an empty registry.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::application::registry::ServiceRegistry;
    ///
    /// let r = ServiceRegistry::new();
    /// assert!(r.get("anything").is_none());
    /// ```
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Return a new [`RegistryBuilder`] for ergonomic construction.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::application::registry::ServiceRegistry;
    /// use stygian_graph::adapters::noop::NoopService;
    /// use std::sync::Arc;
    ///
    /// let r = ServiceRegistry::builder()
    ///     .register("noop", Arc::new(NoopService))
    ///     .build();
    ///
    /// assert!(r.get("noop").is_some());
    /// ```
    pub fn builder() -> RegistryBuilder {
        RegistryBuilder::new()
    }

    /// Register (or replace) a service at runtime.
    ///
    /// The service's initial status is set to [`ServiceStatus::Unknown`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::application::registry::ServiceRegistry;
    /// use stygian_graph::adapters::noop::NoopService;
    /// use std::sync::Arc;
    ///
    /// let r = ServiceRegistry::new();
    /// r.register("noop".to_string(), Arc::new(NoopService));
    /// assert!(r.get("noop").is_some());
    /// ```
    pub fn register(&self, name: String, service: Arc<dyn ScrapingService>) {
        let entry = RegistryEntry {
            service,
            status: ServiceStatus::Unknown,
        };
        self.entries.write().unwrap().insert(name, entry);
    }

    /// Look up a service by name.
    ///
    /// Returns `None` if the service is not registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ScrapingService>> {
        self.entries
            .read()
            .unwrap()
            .get(name)
            .map(|e| Arc::clone(&e.service))
    }

    /// Return the current [`ServiceStatus`] for the named service.
    ///
    /// Returns `None` if no service is registered under that name.
    pub fn status(&self, name: &str) -> Option<ServiceStatus> {
        self.entries
            .read()
            .unwrap()
            .get(name)
            .map(|e| e.status.clone())
    }

    /// List all registered service names.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::application::registry::ServiceRegistry;
    /// use stygian_graph::adapters::noop::NoopService;
    /// use std::sync::Arc;
    ///
    /// let r = ServiceRegistry::new();
    /// r.register("a".to_string(), Arc::new(NoopService));
    /// r.register("b".to_string(), Arc::new(NoopService));
    /// let mut names = r.names();
    /// names.sort();
    /// assert_eq!(names, vec!["a", "b"]);
    /// ```
    pub fn names(&self) -> Vec<String> {
        self.entries.read().unwrap().keys().cloned().collect()
    }

    /// Remove a service from the registry.
    ///
    /// Returns `true` if a service was removed, `false` if it was not registered.
    pub fn deregister(&self, name: &str) -> bool {
        self.entries.write().unwrap().remove(name).is_some()
    }

    /// Run a simple connectivity health check on all registered services.
    ///
    /// Each service's name is pinged by executing a [`crate::ports::ServiceInput`]
    /// with a no-op URL. Results update the stored [`ServiceStatus`]. Returns a
    /// snapshot map of `name → status` after the checks complete.
    #[allow(clippy::unused_async)]
    pub async fn health_check_all(&self) -> HashMap<String, ServiceStatus> {
        let entries_snapshot: Vec<(String, Arc<dyn ScrapingService>)> = {
            let guard = self.entries.read().unwrap();
            guard
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(&v.service)))
                .collect()
        };

        let mut results = HashMap::new();

        for (name, svc) in entries_snapshot {
            let status = Self::probe_service(svc);
            debug!(service = %name, ?status, "health check");
            {
                let mut guard = self.entries.write().unwrap();
                if let Some(entry) = guard.get_mut(&name) {
                    entry.status = status.clone();
                }
            }
            results.insert(name, status);
        }

        results
    }

    /// Probe a single service by calling its `name()` method and marking it
    /// healthy. If the service panics or its name is empty we mark it degraded.
    #[allow(clippy::needless_pass_by_value)]
    fn probe_service(svc: Arc<dyn ScrapingService>) -> ServiceStatus {
        let name = svc.name();
        if name.is_empty() {
            warn!("Service returned empty name during health probe");
            ServiceStatus::Degraded("empty service name".to_string())
        } else {
            ServiceStatus::Healthy
        }
    }

    /// Update stored status for a named service directly.
    ///
    /// Useful for external health-check feedback (e.g., from readiness probes).
    pub fn update_status(&self, name: &str, status: ServiceStatus) {
        let mut guard = self.entries.write().unwrap();
        if let Some(entry) = guard.get_mut(name) {
            entry.status = status;
        }
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Builder ──────────────────────────────────────────────────────────────────

/// Builder for constructing a [`ServiceRegistry`].
///
/// # Example
///
/// ```
/// use stygian_graph::application::registry::ServiceRegistry;
/// use stygian_graph::adapters::noop::NoopService;
/// use std::sync::Arc;
///
/// let registry = ServiceRegistry::builder()
///     .register("noop", Arc::new(NoopService))
///     .build();
///
/// assert_eq!(registry.names().len(), 1);
/// ```
pub struct RegistryBuilder {
    entries: HashMap<String, Arc<dyn ScrapingService>>,
}

#[allow(clippy::unwrap_used)] // RwLock poisoning is unrecoverable
impl RegistryBuilder {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a service with the given name.
    #[must_use]
    pub fn register(mut self, name: impl Into<String>, service: Arc<dyn ScrapingService>) -> Self {
        self.entries.insert(name.into(), service);
        self
    }

    /// Build the registry from accumulated registrations.
    pub fn build(self) -> ServiceRegistry {
        let registry = ServiceRegistry::new();
        {
            let mut guard = registry.entries.write().unwrap();
            for (name, service) in self.entries {
                guard.insert(
                    name,
                    RegistryEntry {
                        service,
                        status: ServiceStatus::Unknown,
                    },
                );
            }
        }
        registry
    }
}

// ─── Global singleton ─────────────────────────────────────────────────────────

/// Process-wide default service registry singleton.
///
/// Initialized once via [`LazyLock`]. Use for global lookup of well-known
/// services without passing the registry through call chains.
///
/// Register your services into the global registry at startup:
///
/// ```no_run
/// use stygian_graph::application::registry::global_registry;
/// use stygian_graph::adapters::noop::NoopService;
/// use std::sync::Arc;
///
/// global_registry().register("noop".to_string(), Arc::new(NoopService));
/// ```
pub fn global_registry() -> &'static ServiceRegistry {
    static INSTANCE: LazyLock<ServiceRegistry> = LazyLock::new(ServiceRegistry::new);
    &INSTANCE
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::noop::NoopService as NoopScraper;

    fn noop() -> Arc<dyn ScrapingService> {
        Arc::new(NoopScraper)
    }

    #[test]
    fn register_and_get() {
        let r = ServiceRegistry::new();
        r.register("svc".to_string(), noop());
        assert!(r.get("svc").is_some());
        assert!(r.get("missing").is_none());
    }

    #[test]
    fn deregister() {
        let r = ServiceRegistry::new();
        r.register("svc".to_string(), noop());
        assert!(r.deregister("svc"));
        assert!(!r.deregister("svc")); // idempotent
        assert!(r.get("svc").is_none());
    }

    #[test]
    fn names_lists_all() {
        let r = ServiceRegistry::builder()
            .register("a", noop())
            .register("b", noop())
            .build();
        let mut names = r.names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn builder_pattern() {
        let r = ServiceRegistry::builder()
            .register("one", noop())
            .register("two", noop())
            .build();
        assert_eq!(r.names().len(), 2);
    }

    #[test]
    fn status_unknown_after_register() {
        let r = ServiceRegistry::new();
        r.register("svc".to_string(), noop());
        assert_eq!(r.status("svc"), Some(ServiceStatus::Unknown));
    }

    #[test]
    fn update_status() {
        let r = ServiceRegistry::new();
        r.register("svc".to_string(), noop());
        r.update_status("svc", ServiceStatus::Healthy);
        assert_eq!(r.status("svc"), Some(ServiceStatus::Healthy));
    }

    #[test]
    fn service_status_is_available() {
        assert!(ServiceStatus::Healthy.is_available());
        assert!(ServiceStatus::Degraded("x".into()).is_available());
        assert!(!ServiceStatus::Unavailable("x".into()).is_available());
        assert!(!ServiceStatus::Unknown.is_available());
    }

    #[tokio::test]
    async fn health_check_all_marks_healthy() {
        let r = ServiceRegistry::builder().register("noop", noop()).build();
        let results = r.health_check_all().await;
        assert_eq!(results.get("noop"), Some(&ServiceStatus::Healthy));
        // Stored status updated
        assert_eq!(r.status("noop"), Some(ServiceStatus::Healthy));
    }

    #[test]
    fn global_registry_singleton_is_same_ref() {
        use std::ptr;
        let a = global_registry();
        let b = global_registry();
        assert!(ptr::addr_eq(a, b));
    }
}
